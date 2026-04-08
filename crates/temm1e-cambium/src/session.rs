//! # Cambium session runner — the reusable end-to-end loop.
//!
//! This is the production entry point for a single Cambium growth session.
//! It is called from:
//!
//! - The `/cambium grow <task>` slash command in `src/main.rs` (Wire 1).
//! - The integration test `tests/real_code_grow_test.rs`.
//! - Future autonomous triggers (Wires 2, 3, 5).
//!
//! Single source of truth: every caller goes through `run_minimal_session`,
//! so the test surface and the production surface exercise identical code.
//!
//! ## What it does
//!
//! 1. Creates an isolated tempdir + minimal Cargo crate (production codebase
//!    is never touched).
//! 2. Builds an `LlmCodeGenerator` from the provided `Provider` + model.
//! 3. Runs the generator against the user's task description.
//! 4. Runs `cargo check`, `cargo clippy --all-targets -- -D warnings`, and
//!    `cargo test` on the result.
//! 5. Returns a `CambiumSessionReport` with full timing, stage results, and
//!    the generated code.
//! 6. The tempdir is dropped at the end of the call (RAII), so cleanup is
//!    automatic even on early return / panic.
//!
//! Optional progress reporting via an `mpsc::UnboundedSender<CambiumProgress>`
//! lets callers stream stage updates to a chat channel in real time.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use temm1e_core::traits::Provider;
use temm1e_core::types::cambium::{GrowthKind, GrowthTrigger};
use temm1e_core::types::error::Temm1eError;
use tokio::process::Command;
use tokio::sync::mpsc::UnboundedSender;
use tokio::time::timeout;

use crate::llm_generator::LlmCodeGenerator;
use crate::pipeline::CodeGenerator;
use crate::sandbox::Sandbox;

/// Maximum wall-clock time for one full session (LLM call + cargo check + clippy + test).
const SESSION_TIMEOUT_SECS: u64 = 300;
/// Maximum time for one cargo subprocess invocation.
const CARGO_TIMEOUT_SECS: u64 = 120;

/// Configuration for a single Cambium session.
#[derive(Debug, Clone)]
pub struct CambiumSessionConfig {
    /// The user-provided task description (the gap to close).
    pub task: String,
    /// LLM model identifier (e.g. "gemini-3-flash-preview", "claude-sonnet-4-6").
    pub model: String,
    /// Maximum files the LLM may write or modify.
    pub max_files: usize,
}

impl CambiumSessionConfig {
    pub fn new(task: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            task: task.into(),
            model: model.into(),
            max_files: 5,
        }
    }
}

/// A progress event emitted as the session runs. Useful for streaming
/// real-time status updates back to a chat channel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CambiumProgress {
    /// Session has been accepted and the sandbox is being prepared.
    Started {
        task: String,
        model: String,
    },
    /// The LLM call is in progress.
    GeneratingCode,
    /// The LLM produced N file changes; about to verify.
    GeneratedFiles {
        count: usize,
    },
    /// `cargo check` started / completed.
    CargoCheckRunning,
    CargoCheckResult {
        passed: bool,
    },
    /// `cargo clippy` started / completed.
    CargoClippyRunning,
    CargoClippyResult {
        passed: bool,
    },
    /// `cargo test` started / completed.
    CargoTestRunning,
    CargoTestResult {
        passed: bool,
        summary: Option<String>,
    },
    /// Session finished with the given outcome.
    Finished {
        success: bool,
        elapsed_ms: u64,
    },
    /// Something went wrong before the verification stages.
    Failed {
        reason: String,
    },
}

/// Final report of a single Cambium session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CambiumSessionReport {
    pub session_id: String,
    pub task: String,
    pub model: String,
    pub started_at: DateTime<Utc>,
    pub completed_at: DateTime<Utc>,
    pub elapsed_ms: u64,
    /// Files the LLM wrote into the sandbox, with their final content.
    pub files_generated: Vec<(PathBuf, String)>,
    pub cargo_check_pass: bool,
    pub cargo_clippy_pass: bool,
    pub cargo_test_pass: bool,
    /// Test summary line, e.g. "test result: ok. 6 passed; 0 failed;".
    pub test_summary: Option<String>,
    /// Whether all gates passed and the session is a success.
    pub success: bool,
    /// On failure, a short reason describing the failing stage.
    pub failure_reason: Option<String>,
}

/// The single entry point that all callers use.
pub async fn run_minimal_session(
    provider: Arc<dyn Provider>,
    config: CambiumSessionConfig,
    progress: Option<UnboundedSender<CambiumProgress>>,
) -> Result<CambiumSessionReport, Temm1eError> {
    let session_id = uuid::Uuid::new_v4().to_string();
    let started_at = Utc::now();
    let started = Instant::now();

    emit(
        &progress,
        CambiumProgress::Started {
            task: config.task.clone(),
            model: config.model.clone(),
        },
    );

    // Wrap the entire session in a timeout so a stuck LLM or hung cargo
    // subprocess cannot block the caller indefinitely.
    let outcome = timeout(
        Duration::from_secs(SESSION_TIMEOUT_SECS),
        run_session_inner(
            provider,
            config.clone(),
            progress.clone(),
            session_id.clone(),
        ),
    )
    .await;

    let total_elapsed = started.elapsed();
    let completed_at = Utc::now();

    match outcome {
        Ok(Ok(mut report)) => {
            report.completed_at = completed_at;
            report.elapsed_ms = total_elapsed.as_millis() as u64;
            emit(
                &progress,
                CambiumProgress::Finished {
                    success: report.success,
                    elapsed_ms: report.elapsed_ms,
                },
            );
            Ok(report)
        }
        Ok(Err(e)) => {
            emit(
                &progress,
                CambiumProgress::Failed {
                    reason: e.to_string(),
                },
            );
            // Return a failed report instead of bubbling the error so the
            // caller always gets structured data.
            Ok(CambiumSessionReport {
                session_id,
                task: config.task,
                model: config.model,
                started_at,
                completed_at,
                elapsed_ms: total_elapsed.as_millis() as u64,
                files_generated: vec![],
                cargo_check_pass: false,
                cargo_clippy_pass: false,
                cargo_test_pass: false,
                test_summary: None,
                success: false,
                failure_reason: Some(e.to_string()),
            })
        }
        Err(_) => {
            let reason = format!("session timed out after {SESSION_TIMEOUT_SECS}s");
            emit(
                &progress,
                CambiumProgress::Failed {
                    reason: reason.clone(),
                },
            );
            Ok(CambiumSessionReport {
                session_id,
                task: config.task,
                model: config.model,
                started_at,
                completed_at,
                elapsed_ms: total_elapsed.as_millis() as u64,
                files_generated: vec![],
                cargo_check_pass: false,
                cargo_clippy_pass: false,
                cargo_test_pass: false,
                test_summary: None,
                success: false,
                failure_reason: Some(reason),
            })
        }
    }
}

async fn run_session_inner(
    provider: Arc<dyn Provider>,
    config: CambiumSessionConfig,
    progress: Option<UnboundedSender<CambiumProgress>>,
    session_id: String,
) -> Result<CambiumSessionReport, Temm1eError> {
    let started_at = Utc::now();

    // 1. Create isolated tempdir + minimal crate.
    let tmp = tempfile::tempdir()
        .map_err(|e| Temm1eError::Tool(format!("Failed to create tempdir: {e}")))?;
    let crate_root = tmp.path().join("cambium-session-crate");
    create_minimal_crate(&crate_root)
        .await
        .map_err(|e| Temm1eError::Tool(format!("Failed to seed crate: {e}")))?;

    // 2. Build the sandbox (no git — local only).
    let sandbox = Sandbox::new(crate_root.clone(), "local".to_string(), "main".to_string());

    // 3. Read the seeded lib.rs to give the LLM context.
    let seed_content = tokio::fs::read_to_string(crate_root.join("src/lib.rs"))
        .await
        .map_err(|e| Temm1eError::Tool(format!("Failed to read seed: {e}")))?;

    // 4. Build the LLM generator with the task wrapped as a manual trigger.
    let generator = LlmCodeGenerator::new(provider, config.model.clone())
        .with_context_file("src/lib.rs".to_string(), seed_content)
        .with_max_files(config.max_files);

    let trigger = GrowthTrigger::Manual {
        description: build_trigger_description(&config.task),
    };

    // 5. Generate code.
    emit(&progress, CambiumProgress::GeneratingCode);
    if let Err(reason) = generator
        .generate(&sandbox, &trigger, &GrowthKind::NewTool)
        .await
    {
        return Ok(CambiumSessionReport {
            session_id,
            task: config.task,
            model: config.model,
            started_at,
            completed_at: Utc::now(),
            elapsed_ms: 0,
            files_generated: vec![],
            cargo_check_pass: false,
            cargo_clippy_pass: false,
            cargo_test_pass: false,
            test_summary: None,
            success: false,
            failure_reason: Some(format!("code generation failed: {reason}")),
        });
    }

    // 6. Collect what was written. The minimal crate has only src/lib.rs;
    //    we read the final content for the report.
    let mut files_generated = Vec::new();
    if let Ok(content) = tokio::fs::read_to_string(crate_root.join("src/lib.rs")).await {
        files_generated.push((PathBuf::from("src/lib.rs"), content));
    }
    emit(
        &progress,
        CambiumProgress::GeneratedFiles {
            count: files_generated.len(),
        },
    );

    // 7. cargo check.
    emit(&progress, CambiumProgress::CargoCheckRunning);
    let (check_ok, _, check_err) = run_cargo(&["check"], &crate_root).await;
    emit(
        &progress,
        CambiumProgress::CargoCheckResult { passed: check_ok },
    );
    if !check_ok {
        return Ok(CambiumSessionReport {
            session_id,
            task: config.task,
            model: config.model,
            started_at,
            completed_at: Utc::now(),
            elapsed_ms: 0,
            files_generated,
            cargo_check_pass: false,
            cargo_clippy_pass: false,
            cargo_test_pass: false,
            test_summary: None,
            success: false,
            failure_reason: Some(format!("cargo check failed: {}", truncate(&check_err, 400))),
        });
    }

    // 8. cargo clippy.
    emit(&progress, CambiumProgress::CargoClippyRunning);
    let (clippy_ok, _, clippy_err) = run_cargo(
        &["clippy", "--all-targets", "--", "-D", "warnings"],
        &crate_root,
    )
    .await;
    emit(
        &progress,
        CambiumProgress::CargoClippyResult { passed: clippy_ok },
    );
    if !clippy_ok {
        return Ok(CambiumSessionReport {
            session_id,
            task: config.task,
            model: config.model,
            started_at,
            completed_at: Utc::now(),
            elapsed_ms: 0,
            files_generated,
            cargo_check_pass: true,
            cargo_clippy_pass: false,
            cargo_test_pass: false,
            test_summary: None,
            success: false,
            failure_reason: Some(format!(
                "cargo clippy failed: {}",
                truncate(&clippy_err, 400)
            )),
        });
    }

    // 9. cargo test.
    emit(&progress, CambiumProgress::CargoTestRunning);
    let (test_ok, test_out, test_err) = run_cargo(&["test"], &crate_root).await;
    let test_summary = test_out
        .lines()
        .find(|l| l.contains("test result:"))
        .map(String::from);
    emit(
        &progress,
        CambiumProgress::CargoTestResult {
            passed: test_ok,
            summary: test_summary.clone(),
        },
    );
    if !test_ok {
        return Ok(CambiumSessionReport {
            session_id,
            task: config.task,
            model: config.model,
            started_at,
            completed_at: Utc::now(),
            elapsed_ms: 0,
            files_generated,
            cargo_check_pass: true,
            cargo_clippy_pass: true,
            cargo_test_pass: false,
            test_summary,
            success: false,
            failure_reason: Some(format!(
                "cargo test failed: {}",
                truncate(
                    if !test_err.is_empty() {
                        &test_err
                    } else {
                        &test_out
                    },
                    400
                )
            )),
        });
    }

    Ok(CambiumSessionReport {
        session_id,
        task: config.task,
        model: config.model,
        started_at,
        completed_at: Utc::now(),
        elapsed_ms: 0,
        files_generated,
        cargo_check_pass: true,
        cargo_clippy_pass: true,
        cargo_test_pass: true,
        test_summary,
        success: true,
        failure_reason: None,
    })
}

/// Wrap the user's task description with explicit context that the LLM
/// needs to produce a complete file.
fn build_trigger_description(task: &str) -> String {
    format!(
        "Modify src/lib.rs to address the following task. \
         You MUST keep the existing `marker()` function and its `marker_works` test exactly as they are. \
         The complete content of src/lib.rs after your change must contain BOTH the existing marker function AND the new code (with tests) you add.\n\n\
         Task: {task}\n\n\
         Requirements:\n\
         - Add at least one #[cfg(test)] test for any new function.\n\
         - Use #[cfg(test)] mod tests block at the bottom of the file.\n\
         - No `unsafe` blocks.\n\
         - No new external dependencies.\n\
         - Code must compile, lint clean (cargo clippy --all-targets -- -D warnings), and pass cargo test."
    )
}

/// Create the minimal isolated Cargo crate that hosts the session.
async fn create_minimal_crate(root: &std::path::Path) -> std::io::Result<()> {
    tokio::fs::create_dir_all(root.join("src")).await?;
    tokio::fs::write(
        root.join("Cargo.toml"),
        "[package]\n\
         name = \"cambium-session-crate\"\n\
         version = \"0.1.0\"\n\
         edition = \"2021\"\n\n\
         [dependencies]\n",
    )
    .await?;
    tokio::fs::write(
        root.join("src/lib.rs"),
        "// Minimal crate seeded by Cambium session runner.\n\
         // The LLM is expected to add code here.\n\n\
         pub fn marker() -> &'static str {\n    \"seeded\"\n}\n\n\
         #[cfg(test)]\n\
         mod tests {\n    use super::*;\n\n    #[test]\n    fn marker_works() {\n        assert_eq!(marker(), \"seeded\");\n    }\n}\n",
    )
    .await?;
    Ok(())
}

/// Run a cargo subprocess in `cwd` with a timeout. Returns
/// (success, stdout, stderr).
async fn run_cargo(args: &[&str], cwd: &std::path::Path) -> (bool, String, String) {
    let cmd_future = Command::new("cargo").args(args).current_dir(cwd).output();
    match timeout(Duration::from_secs(CARGO_TIMEOUT_SECS), cmd_future).await {
        Ok(Ok(o)) => (
            o.status.success(),
            String::from_utf8_lossy(&o.stdout).to_string(),
            String::from_utf8_lossy(&o.stderr).to_string(),
        ),
        Ok(Err(e)) => (false, String::new(), format!("spawn failed: {e}")),
        Err(_) => (
            false,
            String::new(),
            format!("cargo subprocess timed out after {CARGO_TIMEOUT_SECS}s"),
        ),
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        let mut end = max;
        while !s.is_char_boundary(end) && end > 0 {
            end -= 1;
        }
        format!("{}... [truncated]", &s[..end])
    }
}

fn emit(channel: &Option<UnboundedSender<CambiumProgress>>, event: CambiumProgress) {
    if let Some(tx) = channel {
        let _ = tx.send(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_new_uses_defaults() {
        let cfg = CambiumSessionConfig::new("task", "model");
        assert_eq!(cfg.task, "task");
        assert_eq!(cfg.model, "model");
        assert_eq!(cfg.max_files, 5);
    }

    #[test]
    fn truncate_short_unchanged() {
        assert_eq!(truncate("hi", 100), "hi");
    }

    #[test]
    fn truncate_long_marked() {
        let long = "x".repeat(200);
        let t = truncate(&long, 50);
        assert!(t.contains("[truncated]"));
        assert!(t.len() < long.len() + 30);
    }

    #[test]
    fn truncate_utf8_safe() {
        // Multi-byte chars near the boundary must not panic.
        let s = "hello world ẹ ẹ ẹ ẹ ẹ";
        let _t = truncate(s, 15);
    }

    #[test]
    fn build_trigger_includes_task() {
        let desc = build_trigger_description("add a function");
        assert!(desc.contains("add a function"));
        assert!(desc.contains("marker()"));
        assert!(desc.contains("unsafe"));
    }

    #[test]
    fn progress_serializes() {
        let p = CambiumProgress::CargoCheckResult { passed: true };
        let json = serde_json::to_string(&p).unwrap();
        assert!(json.contains("CargoCheckResult"));
    }

    #[test]
    fn report_default_values() {
        let r = CambiumSessionReport {
            session_id: "x".into(),
            task: "t".into(),
            model: "m".into(),
            started_at: Utc::now(),
            completed_at: Utc::now(),
            elapsed_ms: 0,
            files_generated: vec![],
            cargo_check_pass: false,
            cargo_clippy_pass: false,
            cargo_test_pass: false,
            test_summary: None,
            success: false,
            failure_reason: Some("test".into()),
        };
        assert!(!r.success);
        assert!(r.failure_reason.is_some());
    }

    #[tokio::test]
    async fn create_minimal_crate_makes_a_buildable_crate() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        create_minimal_crate(&root).await.unwrap();
        assert!(root.join("Cargo.toml").exists());
        assert!(root.join("src/lib.rs").exists());
        let lib = tokio::fs::read_to_string(root.join("src/lib.rs"))
            .await
            .unwrap();
        assert!(lib.contains("pub fn marker"));
        assert!(lib.contains("marker_works"));
    }
}
