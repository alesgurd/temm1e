# Tem-Code: Implementation Details (Anchoring Reference)

Exact code locations, struct definitions, and line numbers for every modification.

---

## Current Codebase Anchors

### Tool Trait (the contract every tool must satisfy)
**File:** `crates/temm1e-core/src/traits/tool.rs` — 80 lines total

```rust
// Line 6-14: ToolDeclarations
pub struct ToolDeclarations {
    pub file_access: Vec<PathAccess>,
    pub network_access: Vec<String>,
    pub shell_access: bool,
}

// Line 24-28: ToolInput
pub struct ToolInput {
    pub name: String,
    pub arguments: serde_json::Value,
}

// Line 31-35: ToolOutput  
pub struct ToolOutput {
    pub content: String,
    pub is_error: bool,
}

// Line 48-52: ToolContext (WILL BE MODIFIED)
pub struct ToolContext {
    pub workspace_path: std::path::PathBuf,
    pub session_id: String,
    pub chat_id: String,
}

// Line 56-79: Tool trait
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters_schema(&self) -> serde_json::Value;
    fn declarations(&self) -> ToolDeclarations;
    async fn execute(&self, input: ToolInput, ctx: &ToolContext) -> Result<ToolOutput, Temm1eError>;
    fn take_last_image(&self) -> Option<ToolOutputImage> { None }
}
```

### Session State
**File:** `crates/temm1e-core/src/types/session.rs` — 26 lines total

```rust
// Line 7-15: SessionContext (WILL BE MODIFIED)
pub struct SessionContext {
    pub session_id: String,
    pub channel: String,
    pub chat_id: String,
    pub user_id: String,
    pub role: Role,
    pub history: Vec<ChatMessage>,
    pub workspace_path: std::path::PathBuf,
}
```

### Tool Factory
**File:** `crates/temm1e-tools/src/lib.rs`

```rust
// Line 63-73: create_tools signature
pub fn create_tools(
    config: &ToolsConfig,
    channel: Option<Arc<dyn Channel>>,
    pending_messages: Option<PendingMessages>,
    memory: Option<Arc<dyn Memory>>,
    setup_link_gen: Option<Arc<dyn SetupLinkGenerator>>,
    usage_store: Option<Arc<dyn UsageStore>>,
    shared_mode: Option<SharedMode>,
    vault: Option<Arc<dyn Vault>>,
    skill_registry: Option<Arc<RwLock<SkillRegistry>>>,
) -> Vec<Arc<dyn Tool>>

// Line 83-87: File tools registration
if config.file {
    tools.push(Arc::new(FileReadTool::new()));
    tools.push(Arc::new(FileWriteTool::new()));
    tools.push(Arc::new(FileListTool::new()));
}
```

### Existing FileReadTool
**File:** `crates/temm1e-tools/src/file.rs`

```rust
// Line 7: MAX_READ_SIZE = 32 * 1024
// Line 11-16: FileReadTool struct (Default, new())
// Line 21-22: name = "file_read"
// Line 30-40: schema — only "path" param
// Line 51-80: execute — read, truncate at 32KB, return raw content
// Line 258-290: resolve_path() — handles ~, $HOME, absolute, relative
```

### Existing GitTool
**File:** `crates/temm1e-tools/src/git.rs`

```rust
// Line 17-19: VALID_ACTIONS = clone/pull/push/commit/branch/diff/log/status/checkout/add
// Line 31-182: build_args() — constructs git command arguments
// Line 54-79: push — force-push to main/master blocked (line 73)
// Line 185-206: validate_safety() — blocks --hard in extra args
// Line 254-305: execute — tokio::process::Command, timeout, workspace-scoped
```

### Context Builder
**File:** `crates/temm1e-agent/src/context.rs`

```rust
// Line 35-37: HARDCODED CONSTANTS (WILL BE REPLACED)
const MIN_RECENT_MESSAGES: usize = 30;
const MAX_RECENT_MESSAGES: usize = 60;

// Line 41: MEMORY_BUDGET_FRACTION = 0.15
// Line 44: LEARNING_BUDGET_FRACTION = 0.05
// Line 47-49: estimate_tokens() = len / 4
// Line 78-90: build_context() signature
// Line 93-109: Category 1 — System prompt
// Line 112-128: Category 2 — Tool definitions
// Line 130-132: Fixed overhead = 500 tokens
// Line 134-200: Category 3b — Blueprints (10% budget)
// Line 207-217: Category 4 — Recent messages (WILL BE REPLACED)
// Line 222-262: Category 5 — Lambda memory (dynamic)
// Line 265-297: Legacy memory fallback (15%)
// Line 346-389: Category 6 — Learnings (5%)
// Line 392-419: Tool reliability injection (<100 tokens)
// Line 421-480: Category 7 — Older history (fills remainder)
// Line 469-479: Dropped summary injection
// Line 492-499: Chat History Digest
```

### Prompt Optimizer
**File:** `crates/temm1e-agent/src/prompt_optimizer.rs`

```rust
// Line 161-229: build_sections() — determines which sections per tier
// Line 233-272: section_identity() — personality or hardcoded
// Line 275-280: section_tools()
// Line 293-304: section_file_protocol()
// Line 307-333: section_tool_guidelines()
// Line 336-347: section_general_guidelines()
// Line 390-401: section_planning_protocol()
// Line 406-432: section_lambda_memory()
// Line 484-502: build_tiered_system_prompt() — main entry point

// Tier inclusion:
// Minimal: identity only
// Basic: identity + tools + workspace + guidelines
// Standard: all of Basic + file_protocol + tool_guidelines + verification + done_criteria + self_correction + lambda
// Full: all of Standard + planning_protocol
```

### Executor
**File:** `crates/temm1e-agent/src/executor.rs`

```rust
// Line 55-60: execute_tools_parallel() — parallel with dependency grouping
// Line 42: DEFAULT_MAX_CONCURRENT = 5
// Tool lookup: tools.iter().find(|t| t.name() == tool_name) — first-match-wins
// Dependency detection groups independent tools for parallel execution
```

### Model Registry
**File:** `crates/temm1e-core/src/types/model_registry.rs`

```rust
// model_limits(model) -> (context_window, max_output_tokens)
// Claude Sonnet/Opus 4.6: (200_000, 16_384)
// GPT-5.4: (1_050_000, 32_768)
// Gemini 3-Flash: (1_048_576, 65_536)
// Grok-4: (2_000_000, 32_768)
// Default: (128_000, 4_096)
```

### ToolContext Construction Sites (must add read_tracker)

```
crates/temm1e-agent/src/executor.rs      — execute_tool(), execute_tools_parallel()
crates/temm1e-agent/src/runtime.rs       — process_message() tool dispatch
crates/temm1e-agent/src/streaming.rs     — streaming tool execution
crates/temm1e-cores/src/runtime.rs       — CoreRuntime tool dispatch
crates/temm1e-cores/src/invoke_tool.rs   — InvokeCoreTool dispatch
crates/temm1e-tools/src/git.rs:600-604   — test helper
crates/temm1e-test-utils/src/lib.rs      — test mocks
```

Each needs `read_tracker: None` (or `read_tracker: Some(tracker.clone())` for coding contexts).

---

## New Dependencies

### Phase 1 (Foundation)
```toml
# crates/temm1e-tools/Cargo.toml additions:
glob = "0.3"        # For CodeGlobTool
ignore = "0.4"      # For gitignore-aware walking (CodeGlobTool, CodeGrepTool)
regex = "1"         # Already in workspace (for CodeGrepTool patterns)
```

### Phase 3 (Intelligence)
```toml
# Feature-gated tree-sitter:
[features]
code-analysis = ["tree-sitter", "tree-sitter-rust", "tree-sitter-python", ...]

[dependencies]
tree-sitter = { version = "0.24", optional = true }
tree-sitter-rust = { version = "0.24", optional = true }
# ... per language
```

---

## Shared Utilities

### resolve_path() — Reuse from file.rs
**File:** `crates/temm1e-tools/src/file.rs` (lines 258-290)

Currently private to the file module. Must be made `pub(crate)` so code_edit, code_glob, code_grep can use it.

**Change:** `fn resolve_path(...)` → `pub(crate) fn resolve_path(...)`

### atomic_write() — New utility
**Location:** `crates/temm1e-tools/src/code_edit.rs` (private to module)

```rust
async fn atomic_write(path: &std::path::Path, content: &[u8]) -> Result<(), Temm1eError> {
    let tmp = path.with_extension("temm1e.tmp");
    tokio::fs::write(&tmp, content).await.map_err(|e| {
        Temm1eError::Tool(format!("Failed to write temp file: {}", e))
    })?;
    tokio::fs::rename(&tmp, path).await.map_err(|e| {
        // Clean up temp file on rename failure
        let _ = std::fs::remove_file(&tmp);
        Temm1eError::Tool(format!("Failed to rename temp file: {}", e))
    })?;
    Ok(())
}
```

---

## Test Strategy

Each new tool needs:
1. `test_name()` — verify tool name is correct
2. `test_parameters_schema_valid_json()` — verify schema is valid JSON Schema
3. `test_declarations()` — verify resource declarations
4. `test_execute_*()` — functional tests with tempdir

Integration tests:
- Multi-tool sequence: file_read → code_edit → file_read (verify edit applied)
- Read-before-write gate: code_edit without prior file_read → error
- code_glob on real directory → correct results
- code_grep with regex → correct matches
- code_snapshot create/restore cycle → files restored correctly
- code_patch multi-file → all edits applied atomically
- Dynamic recent budget: verify token-based not count-based allocation
