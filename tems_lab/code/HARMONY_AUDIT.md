# Tem-Code: Harmony Audit Results

**Date:** 2026-04-10
**Status:** CLEAR — Zero conflicts across all 23 crates

## Existing Tool Names (collision check)

These names are TAKEN — new tools must not reuse them:

```
shell, file_read, file_write, file_list, git, web_fetch,
send_message, send_file, check_messages, memory_manage,
lambda_recall, key_manage, usage_audit, mode_switch,
use_skill, browser, desktop, self_create_tool,
invoke_core, mcp_manage, self_extend, self_add_mcp
```

New tool names (verified unique):
- `code_edit` — no collision
- `code_glob` — no collision
- `code_grep` — no collision
- `code_patch` — no collision
- `code_snapshot` — no collision
- `code_search` — no collision (Phase 3)

## Subsystem Compatibility

### Tool Registration (`temm1e-tools/src/lib.rs`)
- Tools are `Vec<Arc<dyn Tool>>` built in `create_tools()`
- New tools: add to Vec conditionally on `config.file` (same gate as file_read/write)
- Registration is additive — push to Vec before `AgentRuntime::with_limits()`
- **No breaking change**

### Tool Trait (`temm1e-core/src/traits/tool.rs`)
- 5 required methods: `name()`, `description()`, `parameters_schema()`, `declarations()`, `execute()`
- 1 optional: `take_last_image()` (default None)
- ToolContext: `{ workspace_path, session_id, chat_id }` — immutable ref
- ToolOutput: `{ content: String, is_error: bool }`
- **New tools just implement the trait — no changes to trait needed**

### Read-Before-Write Gate
- ToolContext is `&ToolContext` (immutable). Tools cannot mutate session state.
- **Solution:** Add `read_tracker: Option<Arc<RwLock<HashSet<PathBuf>>>>` to ToolContext
- Existing tools ignore it (it's Option). New code_edit checks it.
- This is the ONE field addition to a shared struct. All existing tools unaffected because they don't access the field.

### Executor (`temm1e-agent/src/executor.rs`)
- `execute_tool()` finds tool by name (first-match-wins linear search)
- `execute_tools_parallel()` groups by dependency, runs concurrently (max 5)
- Dependency detection: shell calls = mutually dependent, file_read = independent
- **New tools automatically inherit:** sandboxing, RBAC, argument validation, parallel execution

### TemDOS Cores (`temm1e-cores/src/invoke_tool.rs`)
- `filtered_tools()` gives cores all tools EXCEPT `invoke_core`
- New coding tools automatically visible to Architecture, Code-Review, Test, Debug cores
- **No change needed — perfect for architect/editor pattern**

### Swarm/Hive (`temm1e-hive/src/worker.rs`)
- Workers get `tools.clone()` — new tools automatically available
- **No change needed**

### Perpetuum (`temm1e-perpetuum/src/tools.rs`)
- Has its OWN isolated tool set (create_alarm, create_monitor, etc.)
- **Completely independent — no interaction**

### Cambium (`temm1e-cambium/src/sandbox.rs`)
- Uses git branches for code sandbox, not worktrees
- Doesn't reference tool names in generated code
- **No conflict with worktree isolation for agent sessions**

### Anima/Personality (`temm1e-anima/src/personality.rs`)
- Replaces identity section in system prompt via `generate_identity_section()`
- No budget conflict — personality IS the system prompt identity, not an addition
- **No change needed**

### Consciousness Engine (`temm1e-agent/src/consciousness_engine.rs`)
- Separate LLM calls (pre/post observation)
- Does NOT consume main context budget
- **No interaction with new tools or context changes**

### Eigen-Tune (`temm1e-distill`)
- Fire-and-forget hooks after provider calls + tool execution
- **Zero overhead, no interaction**

### MCP Bridge (`temm1e-mcp/src/bridge.rs`)
- Has `resolve_display_name()` for collision resolution
- MCP tools namespaced separately from built-in tools
- **No collision risk**

### All Providers (anthropic.rs, openai_compat.rs, gemini.rs)
- Serialize `Vec<ToolDefinition>` as JSON — no limit on tool count in code
- Each new tool adds ~200 tokens to tool definitions
- 7 new tools × 200 = ~1400 tokens additional tool definition overhead
- **Within acceptable range — context budget absorbs this**

### Gateway Sessions (`temm1e-gateway/src/session.rs`)
- SessionContext cloned via `.cloned()` — adding fields won't break
- MAX_SESSIONS = 1000, MAX_HISTORY_PER_SESSION = 200
- **Adding read_tracker field: use Arc<RwLock<>> to avoid clone cost**

### Recovery System (`temm1e-agent/src/recovery.rs`)
- Checkpoints are JSON-serialized history — separate from SessionContext
- **New checkpoint system (git write-tree) is orthogonal — doesn't conflict**

### Circuit Breaker (`temm1e-agent/src/circuit_breaker.rs`)
- Provider-only (API failures). New tools don't trigger it.
- Tool failures handled by executor's max_consecutive_failures (2)
- **No interaction**

### Budget Tracker (`temm1e-agent/src/budget.rs`)
- Tracks LLM API costs only (input + output tokens)
- New tools don't incur tracked costs (no provider calls)
- **No interaction**

### Credential Scrubbing (`temm1e-tools/src/credential_scrub.rs`)
- Applied to all tool outputs by runtime
- **New tools automatically inherit credential scrubbing**

### Context Builder (`temm1e-agent/src/context.rs`)
- Budget categories are elastic (lambda_memory absorbs remainder)
- Adding repo_map budget (4%) fits cleanly between blueprints (10%) and lambda
- Hardcoded MIN/MAX_RECENT_MESSAGES → convert to token-budgeted fraction
- **Compatible — follows existing budget slot pattern**

### Complexity Classifier (`temm1e-agent/src/model_router.rs`)
- Read-only tools stay Simple. Write tools trigger Order/Complex.
- New `code_edit` has write semantics → Order classification (correct)
- **No keyword additions needed — existing heuristics handle it**

## Files to Modify (Complete List)

### Phase 1: Foundation

| File | Change | Risk |
|------|--------|------|
| `temm1e-core/src/traits/tool.rs` | Add `read_tracker` field to ToolContext | LOW — Optional field, existing tools ignore |
| `temm1e-core/src/types/session.rs` | Add `read_tracker` field to SessionContext | LOW — Arc-wrapped, clone-safe |
| `temm1e-tools/src/lib.rs` | Add new tool modules + register in create_tools() | ZERO — Additive |
| `temm1e-tools/src/code_edit.rs` | NEW FILE — CodeEditTool | ZERO — New file |
| `temm1e-tools/src/code_glob.rs` | NEW FILE — CodeGlobTool | ZERO — New file |
| `temm1e-tools/src/code_grep.rs` | NEW FILE — CodeGrepTool | ZERO — New file |
| `temm1e-tools/src/code_patch.rs` | NEW FILE — CodePatchTool | ZERO — New file |
| `temm1e-tools/src/code_snapshot.rs` | NEW FILE — CodeSnapshotTool | ZERO — New file |
| `temm1e-tools/src/git.rs` | Add safety intercepts (--no-verify strip, --amend block) | LOW — Additive to validate_safety() |
| `temm1e-agent/src/context.rs` | Replace MIN/MAX_RECENT_MESSAGES with token-budgeted fraction | LOW — Same algorithm, different trigger |
| `temm1e-agent/src/prompt_optimizer.rs` | Add coding instructions to tiered prompts | ZERO — Additive text |

### Phase 2: Safety

| File | Change | Risk |
|------|--------|------|
| `temm1e-agent/src/executor.rs` | Add guardrail intercept before tool dispatch | LOW — Pre-existing validation pattern |
| `temm1e-agent/src/executor.rs` | Auto-checkpoint before code_edit dispatch | LOW — Additional call, non-blocking |

### Phase 3: Intelligence

| File | Change | Risk |
|------|--------|------|
| `temm1e-tools/Cargo.toml` | Add tree-sitter dependencies (feature-gated) | ZERO — Optional feature |
| `temm1e-tools/src/code_search.rs` | NEW FILE — CodeSearchTool | ZERO — New file |
| `temm1e-agent/src/context.rs` | Add repo_map budget category | LOW — Same pattern as blueprint budget |
| `temm1e-agent/src/runtime.rs` | Add deliberation step for Complex tasks | LOW — Extra provider call before tool loop |

## Risk Assessment: ZERO RISK

- All changes are additive (new files, new Vec entries, new budget categories)
- The one shared-struct change (ToolContext) uses Optional field — existing code unaffected
- No existing behavior is modified — only new behavior is added
- No existing tool is renamed, removed, or has its API changed
- All new tools follow the exact same trait pattern as existing tools
- Context budget changes follow the exact same priority-based pattern
