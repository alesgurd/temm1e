# Tem-Code: Implementation Plan

**Date:** 2026-04-10
**Branch:** `tem-code`
**Risk Level:** ZERO — all changes additive, harmony audit clear

---

## Phase 1: Foundation (Core Tools + Context Fix)

### 1.1 ToolContext Enhancement

**File:** `crates/temm1e-core/src/traits/tool.rs`

**Current ToolContext (line 48-52):**
```rust
pub struct ToolContext {
    pub workspace_path: std::path::PathBuf,
    pub session_id: String,
    pub chat_id: String,
}
```

**Change:** Add optional read tracker:
```rust
pub struct ToolContext {
    pub workspace_path: std::path::PathBuf,
    pub session_id: String,
    pub chat_id: String,
    /// Tracks which files have been read in this session.
    /// Used by code_edit to enforce read-before-write gate.
    /// None for non-coding contexts (backwards compatible).
    pub read_tracker: Option<std::sync::Arc<tokio::sync::RwLock<std::collections::HashSet<std::path::PathBuf>>>>,
}
```

**Propagation:** Every place that constructs a ToolContext needs to pass `read_tracker: None` (or the tracker). Grep for `ToolContext {` to find all construction sites.

**SessionContext** (`crates/temm1e-core/src/types/session.rs`): Add matching field:
```rust
pub struct SessionContext {
    // ... existing fields ...
    pub read_tracker: std::sync::Arc<tokio::sync::RwLock<std::collections::HashSet<std::path::PathBuf>>>,
}
```
Initialize as `Arc::new(RwLock::new(HashSet::new()))` in session creation.

---

### 1.2 CodeEditTool

**File:** `crates/temm1e-tools/src/code_edit.rs` (NEW)

**Tool name:** `code_edit`
**Parameters:**
- `file_path: String` — absolute or relative path
- `old_string: String` — exact text to find (must be unique in file)
- `new_string: String` — replacement text (must differ from old_string)
- `replace_all: bool` — replace all occurrences (default: false)

**Algorithm:**
1. Resolve path via `resolve_path()` (reuse from file.rs)
2. Check read_tracker — if file not in set, return error: "File must be read before editing. Use file_read first."
3. Read current file content
4. Find `old_string` in content:
   - Not found → return error with closest fuzzy match suggestion
   - Found multiple times (and replace_all=false) → return error with match count and line numbers
   - Found once (or replace_all=true) → proceed
5. Replace old_string with new_string
6. Write via atomic temp file + rename:
   - Write to `{path}.temm1e.tmp`
   - `fs::rename()` (atomic on POSIX)
   - On Windows: use `fs::rename` with retry (or platform-specific atomic)
7. Return success with diff summary (lines changed, bytes delta)

**Declarations:** `file_access: [ReadWrite(".")]`, `network_access: []`, `shell_access: false`

**Tests:**
- Unique match → replace succeeds
- No match → error with suggestion
- Multiple matches + replace_all=false → error with count
- Multiple matches + replace_all=true → all replaced
- Read-before-write gate → error if not read
- Atomic write → no corruption on simulated crash
- old_string == new_string → error (no-op prevention)

---

### 1.3 Enhanced FileReadTool (code_read behavior)

**File:** `crates/temm1e-tools/src/file.rs`

**Changes to existing FileReadTool:**
- Add `offset` param (line number to start from, 0-indexed, default 0)
- Add `limit` param (max lines to return, default 2000)
- Output format: line-numbered (`{line_num}\t{content}`)
- After successful read: insert resolved path into read_tracker (if present)
- Keep backwards compatibility: if no offset/limit provided, works as before (but with line numbers)

**Current schema (line 30-40):**
```json
{ "type": "object", "properties": { "path": { ... } }, "required": ["path"] }
```

**New schema:**
```json
{
  "type": "object",
  "properties": {
    "path": { "type": "string", "description": "File path to read" },
    "offset": { "type": "integer", "description": "Start line (0-indexed, default: 0)" },
    "limit": { "type": "integer", "description": "Max lines to return (default: 2000)" }
  },
  "required": ["path"]
}
```

**Output change:** From raw content to line-numbered:
```
1	use std::io;
2	use std::fs;
3	
4	fn main() {
```

**read_tracker integration:** After successful read, if `ctx.read_tracker.is_some()`:
```rust
if let Some(tracker) = &ctx.read_tracker {
    tracker.write().await.insert(resolved_path.clone());
}
```

---

### 1.4 CodeGlobTool

**File:** `crates/temm1e-tools/src/code_glob.rs` (NEW)

**Tool name:** `code_glob`
**Parameters:**
- `pattern: String` — glob pattern (e.g., "**/*.rs", "src/**/*.ts")
- `path: String` — base directory (default: workspace root)

**Implementation:**
- Use `glob` crate (already in Cargo ecosystem, lightweight)
- Respect `.gitignore` via `ignore` crate (same as ripgrep uses)
- Sort results by modification time (newest first)
- Limit: max 500 results, return count if exceeded

**Declarations:** `file_access: [Read(".")]`, `network_access: []`, `shell_access: false`

---

### 1.5 CodeGrepTool

**File:** `crates/temm1e-tools/src/code_grep.rs` (NEW)

**Tool name:** `code_grep`
**Parameters:**
- `pattern: String` — regex pattern
- `path: String` — search directory (default: workspace root)
- `glob: String` — file filter (e.g., "*.rs")
- `output_mode: String` — "content" | "files_with_matches" | "count" (default: "files_with_matches")
- `head_limit: usize` — max results (default: 250)
- `context: usize` — lines of context around matches (default: 0)
- `case_insensitive: bool` — default: false

**Implementation:**
- Use `grep-regex` + `grep-searcher` crates (ripgrep's libraries) OR
- Use `regex` crate + manual file walking with `ignore` crate for gitignore
- The simpler approach: `regex` + `ignore::WalkBuilder` (fewer deps, sufficient)

**Declarations:** `file_access: [Read(".")]`, `network_access: []`, `shell_access: false`

---

### 1.6 Git Safety Enhancement

**File:** `crates/temm1e-tools/src/git.rs`

**Changes to existing `validate_safety()` (line 185-206):**

Add these checks:
```rust
fn validate_safety(action: &str, args: &serde_json::Value) -> Result<(), Temm1eError> {
    // EXISTING: Block reset --hard in extra args
    
    // NEW: Block --no-verify on commit
    if action == "commit" {
        if let Some(extra) = args.get("args").and_then(|v| v.as_array()) {
            for a in extra {
                if let Some(s) = a.as_str() {
                    if s == "--no-verify" {
                        return Err(Temm1eError::Tool(
                            "--no-verify is blocked. Pre-commit hooks exist for a reason.".into(),
                        ));
                    }
                }
            }
        }
    }
    
    // NEW: Block --amend (create new commits instead)
    if action == "commit" {
        if let Some(extra) = args.get("args").and_then(|v| v.as_array()) {
            for a in extra {
                if let Some(s) = a.as_str() {
                    if s == "--amend" {
                        return Err(Temm1eError::Tool(
                            "--amend blocked by default. Create a new commit instead. \
                             Amending modifies the previous commit which may destroy work.".into(),
                        ));
                    }
                }
            }
        }
    }
    
    Ok(())
}
```

**Changes to `build_args()` for `add` action (line 134-158):**

Block `git add .` and `git add -A` in system prompt (soft guardrail). Keep the code as-is for backwards compat but add a prompt instruction to prefer named files.

---

### 1.7 Dynamic Recent History Budget

**File:** `crates/temm1e-agent/src/context.rs`

**Current (lines 35-37):**
```rust
const MIN_RECENT_MESSAGES: usize = 30;
const MAX_RECENT_MESSAGES: usize = 60;
```

**Replace with:**
```rust
/// Fraction of total context budget allocated to recent conversation history.
const RECENT_BUDGET_FRACTION: f32 = 0.25;

/// Absolute minimum: always keep last user message + last assistant response.
/// This ensures the current query is never dropped.
const MIN_RECENT_MESSAGES: usize = 2;
```

**Current recent message selection (lines 207-217):**
```rust
let recent_count = history.len()
    .min(MAX_RECENT_MESSAGES)
    .max(history.len().min(MIN_RECENT_MESSAGES));
let recent_start = history.len().saturating_sub(recent_count);
let recent_messages: Vec<ChatMessage> = history[recent_start..].to_vec();
let recent_tokens: usize = recent_messages.iter().map(estimate_message_tokens).sum();
```

**Replace with token-budgeted selection:**
```rust
// Dynamic recent budget: fraction of skull, not fixed message count
let recent_budget = ((budget as f32) * RECENT_BUDGET_FRACTION) as usize;

// Walk backward from newest, keeping atomic turns together
let recent_turns = group_into_turns(history);
let mut recent_indices: Vec<usize> = Vec::new();
let mut recent_tokens: usize = 0;

for turn in recent_turns.iter().rev() {
    let turn_tokens: usize = turn.indices.iter()
        .map(|&i| estimate_message_tokens(&history[i]))
        .sum();
    if recent_tokens + turn_tokens > recent_budget && recent_indices.len() >= MIN_RECENT_MESSAGES {
        break;
    }
    recent_tokens += turn_tokens;
    recent_indices.extend_from_slice(&turn.indices);
}
recent_indices.sort_unstable();

let recent_messages: Vec<ChatMessage> = recent_indices.iter()
    .map(|&i| history[i].clone())
    .collect();
let recent_start = recent_indices.first().copied().unwrap_or(history.len());
```

**Why this works:**
- On 200K model: 25% = 50K tokens for recent → generous
- On 128K model: 25% = 32K tokens → still generous  
- On 2M model: 25% = 500K tokens → massive window
- Atomic turn grouping preserved (tool_use + tool_result kept together)
- Minimum 2 messages ensures current query never dropped
- Same algorithm as older history (lines 448-459) — consistent skull philosophy

---

### 1.8 System Prompt Coding Instructions

**File:** `crates/temm1e-agent/src/prompt_optimizer.rs`

**Add new section after `section_lambda_memory()` (line 432):**

```rust
fn section_coding_tools() -> String {
    "## Coding Tools\n\
     When working with code, prefer these specialized tools:\n\
     - Use `file_read` to read files (returns line-numbered output with offset/limit)\n\
     - Use `code_edit` to modify files (exact string replacement, read-before-edit enforced)\n\
     - Use `code_glob` to find files by pattern (gitignore-aware, result-limited)\n\
     - Use `code_grep` to search file contents (regex, output modes, result-limited)\n\
     - Use `git` for version control (force-push blocked, safety checks enforced)\n\
     - Use `code_snapshot` to checkpoint/restore file state\n\n\
     Prefer `code_edit` over `file_write` for modifying existing files — it's safer \
     (read-before-edit enforced) and more token-efficient (only changed portion transmitted).\n\
     Prefer `code_grep` over `shell` for searching — it limits output and respects .gitignore.\n\
     Prefer `code_glob` over `shell` for finding files — it limits results and sorts by recency.\n\n\
     ## Git Best Practices\n\
     - Work on feature branches, not main/master\n\
     - Commit with descriptive messages focused on WHY not WHAT\n\
     - Stage specific files by name (git add with files array)\n\
     - Run tests before pushing\n\
     - Create new commits rather than amending\n"
    .to_string()
}
```

**Include in Standard + Full tiers** (add to `build_sections()` lines 195, 215).

---

### 1.9 CodePatchTool

**File:** `crates/temm1e-tools/src/code_patch.rs` (NEW)

**Tool name:** `code_patch`
**Parameters:**
- `changes: Array<{ file_path: String, edits: Array<{ old_string: String, new_string: String }> }>`

**Algorithm:**
1. Validate ALL edits can apply (dry run — find all old_strings without modifying)
2. If any validation fails → return error listing all failures
3. Apply all edits atomically (all succeed or all rollback via backup)
4. Return summary of all changes

---

### 1.10 CodeSnapshotTool

**File:** `crates/temm1e-tools/src/code_snapshot.rs` (NEW)

**Tool name:** `code_snapshot`
**Parameters:**
- `action: String` — "create" | "restore" | "list" | "diff"
- `name: String` — human-readable name (for create)
- `snapshot_id: String` — ID (for restore/diff)

**Implementation:**
- `create`: `git write-tree` captures current state → store tree hash + name + timestamp
- `restore`: `git read-tree {hash} && git checkout-index -a -f` restores files
- `list`: return stored snapshots with names and timestamps
- `diff`: `git diff-tree -r {hash} HEAD` shows changes since snapshot
- Storage: JSON file at `{workspace}/.temm1e/snapshots.json` (gitignored)

---

### 1.11 Tool Registration

**File:** `crates/temm1e-tools/src/lib.rs`

**Add modules (after line 29):**
```rust
mod code_edit;
mod code_glob;
mod code_grep;
mod code_patch;
mod code_snapshot;
```

**Add exports (after line 48):**
```rust
pub use code_edit::CodeEditTool;
pub use code_glob::CodeGlobTool;
pub use code_grep::CodeGrepTool;
pub use code_patch::CodePatchTool;
pub use code_snapshot::CodeSnapshotTool;
```

**Add to create_tools() (after line 87, inside `if config.file` block):**
```rust
if config.file {
    tools.push(Arc::new(FileReadTool::new()));
    tools.push(Arc::new(FileWriteTool::new()));
    tools.push(Arc::new(FileListTool::new()));
    // Tem-Code enhanced tools
    tools.push(Arc::new(CodeEditTool::new()));
    tools.push(Arc::new(CodeGlobTool::new()));
    tools.push(Arc::new(CodeGrepTool::new()));
    tools.push(Arc::new(CodePatchTool::new()));
    tools.push(Arc::new(CodeSnapshotTool::new()));
}
```

---

## Phase 2: Safety (Self-Governing Guardrails)

### 2.1 Executor Guardrails

**File:** `crates/temm1e-agent/src/executor.rs`

Add pre-dispatch check in `execute_tool()`:
- Before `tool.execute()`, check for dangerous shell commands
- Auto-stash before `git reset --hard` (if it somehow passes git tool's own safety)
- Auto-checkpoint before `code_edit` calls

### 2.2 Worktree Isolation for Cores

**File:** `crates/temm1e-cores/src/runtime.rs`

When a core is dispatched for a coding task:
1. `git worktree add /tmp/temm1e-core-{hash} -b tem/core/{task}`
2. Override core's ToolContext.workspace_path to worktree path
3. Core executes in isolation
4. On completion: collect diff, merge on success, cleanup worktree

---

## Phase 3: Intelligence (Tree-sitter + Repo Map)

### 3.1 Tree-sitter Integration

**New module in `crates/temm1e-tools/src/treesitter.rs`** (or new crate if deps are heavy)

Dependencies (feature-gated: `--features code-analysis`):
```toml
[dependencies]
tree-sitter = { version = "0.24", optional = true }
tree-sitter-rust = { version = "0.24", optional = true }
tree-sitter-python = { version = "0.24", optional = true }
tree-sitter-javascript = { version = "0.24", optional = true }
tree-sitter-typescript = { version = "0.24", optional = true }
tree-sitter-go = { version = "0.24", optional = true }
```

### 3.2 Repo Map in Context Builder

**File:** `crates/temm1e-agent/src/context.rs`

Add after blueprint injection (line 199), before lambda memory (line 222):
```rust
const REPO_MAP_BUDGET_FRACTION: f32 = 0.04;
let repo_map_budget = ((budget as f32) * REPO_MAP_BUDGET_FRACTION) as usize;
// Generate repo map using tree-sitter (if available)
// Inject as System message
```

### 3.3 Internal Deliberation

**File:** `crates/temm1e-agent/src/runtime.rs`

In `process_message()`, when `complexity == Complex`:
- Before tool loop, make one extra `provider.complete()` call with deliberation prompt
- Store result in task state (not shown to user)
- Feed deliberation output into context for the main tool loop

---

## Dependency Order

```
1.1 ToolContext change        ← MUST be first (other tools depend on it)
   ↓
1.2 CodeEditTool              ← Depends on ToolContext.read_tracker
1.3 FileReadTool enhancement  ← Populates read_tracker
   ↓ (can be parallel with 1.2-1.3)
1.4 CodeGlobTool
1.5 CodeGrepTool
1.6 Git safety
1.9 CodePatchTool             ← Depends on CodeEditTool (reuses logic)
1.10 CodeSnapshotTool
   ↓
1.7 Dynamic recent budget     ← Independent, can be any time in Phase 1
1.8 System prompt instructions ← After tools exist (references them by name)
1.11 Tool registration        ← After all tools implemented
```

## Compilation Gates

After each implementation step:
```bash
cargo check --workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all -- --check
cargo test --workspace
```

ALL must pass before moving to next step.
