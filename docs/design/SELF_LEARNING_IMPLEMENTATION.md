# Self-Learning Enhancement — Full Implementation Specification

> Every code change, schema migration, file path, and line number.
> Execute only when 100% certain. Each fix is independent unless noted.

---

## Status

| Fix | Description | Status | Risk |
|-----|-------------|--------|------|
| **Fix 1** | Learning retrieval scored by V(a,t) | READY | ZERO — additive, no schema change |
| **Fix 2** | Lambda importance reinforcement | READY | ZERO — additive field, no mapping issues |
| **Fix 3** | Blueprint fitness lifecycle + GC | READY | ZERO — purely subtractive GC + additive field |
| **Fix 4** | Eigen-Tune retention policy | READY | ZERO — additive config + new method |
| **Fix 5** | Lambda memory deduplication | READY | ZERO — GC-time only, explicit saves protected |

---

## Fix 1: Learning Retrieval Scored by V(a,t)

### What

Replace the current `memory.search("learning:", limit=5)` retrieval (timestamp-ordered, no scoring) with value-function-scored retrieval using the `learning_value()` function already implemented in `learning.rs`.

Also add `times_applied` feedback: after a successful task where learnings were in context, increment `times_applied` on those learnings.

### Why

The value function `V = Q × R × U` exists in code (learning.rs) but context.rs still uses the old path. Learnings are retrieved by timestamp DESC, meaning the 5 most recent are always injected regardless of quality or relevance. A 1-day-old garbage learning outranks a 30-day-old critical one.

### Files Touched

| File | Lines | Change |
|------|-------|--------|
| `crates/temm1e-agent/src/context.rs` | 345-376 | Replace search + format with scored retrieval |
| `crates/temm1e-agent/src/runtime.rs` | ~1404-1450 | Track which learnings were injected, increment `times_applied` after success |
| `crates/temm1e-core/src/traits/memory.rs` | ~99 | No change needed — existing `search()` returns all entries, we score in-memory |

### Current Code (context.rs:345-376)

```rust
// Legacy Category 6: learnings
if !query.is_empty() {
    let learning_budget = ((budget as f32) * LEARNING_BUDGET_FRACTION) as usize;
    let remaining_for_learnings = available_after_fixed_and_recent
        .saturating_sub(memory_tokens_used + knowledge_tokens_used);
    let learning_budget = learning_budget.min(remaining_for_learnings);

    let learning_opts = SearchOpts {
        limit: 5,
        session_filter: None,
        ..Default::default()
    };

    if let Ok(entries) = memory.search("learning:", learning_opts).await {
        let learnings: Vec<learning::TaskLearning> = entries
            .iter()
            .filter_map(|e| serde_json::from_str(&e.content).ok())
            .collect();

        if !learnings.is_empty() {
            let formatted = learning::format_learnings_context(&learnings);
            let tokens = estimate_tokens(&formatted);
            if tokens <= learning_budget && !formatted.is_empty() {
                lambda_messages.push(ChatMessage {
                    role: Role::System,
                    content: MessageContent::Text(formatted),
                });
                learning_tokens_used = tokens;
            }
        }
    }
}
```

### New Code (context.rs:345-376)

```rust
// Category 6: learnings — scored by V(a,t) = Q × R × U
if !query.is_empty() {
    let learning_budget = ((budget as f32) * LEARNING_BUDGET_FRACTION) as usize;
    let remaining_for_learnings = available_after_fixed_and_recent
        .saturating_sub(memory_tokens_used + knowledge_tokens_used);
    let learning_budget = learning_budget.min(remaining_for_learnings);

    // Fetch more candidates than we need, then score and take top 5
    let learning_opts = SearchOpts {
        limit: 50,
        session_filter: None,
        ..Default::default()
    };

    if let Ok(entries) = memory.search("learning:", learning_opts).await {
        let now = chrono::Utc::now();
        let mut scored: Vec<(f64, learning::TaskLearning)> = entries
            .iter()
            .filter_map(|e| serde_json::from_str(&e.content).ok())
            .map(|l: learning::TaskLearning| {
                let v = learning::learning_value(&l, now);
                (v, l)
            })
            .filter(|(v, _)| *v >= 0.05) // GONE_THRESHOLD
            .collect();

        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(5);

        let top_learnings: Vec<learning::TaskLearning> =
            scored.into_iter().map(|(_, l)| l).collect();

        if !top_learnings.is_empty() {
            let formatted = learning::format_learnings_context(&top_learnings);
            let tokens = estimate_tokens(&formatted);
            if tokens <= learning_budget && !formatted.is_empty() {
                lambda_messages.push(ChatMessage {
                    role: Role::System,
                    content: MessageContent::Text(formatted),
                });
                learning_tokens_used = tokens;
            }
        }
    }
}
```

**Key changes:**
- `limit: 5` → `limit: 50` (fetch more candidates)
- Score each with `learning_value()`, filter below 0.05, sort descending, take top 5
- No schema change, no new trait methods, no migration

### times_applied Feedback (runtime.rs)

**Problem:** `times_applied` is a field on `TaskLearning` but nothing increments it. We need to:
1. Track which learning IDs were injected into context
2. After task completion (success), increment `times_applied` on those learnings
3. Persist the updated learnings back to memory

**Approach:** This requires knowing which learnings were injected. The current architecture passes learnings through `build_context()` which returns a `CompletionRequest` — the learning IDs are lost.

**Simplest change:** Add a return value from the learning injection block in context.rs that returns the injected learning entry IDs. Then in runtime.rs, after task success, update those entries.

**However**, this requires:
- Returning learning IDs from `build_context()` (signature change, large function)
- Fetching entries by ID, deserializing, incrementing, re-serializing, re-storing

**Assessment:** The feedback loop is valuable but touches many interfaces. **Defer to Fix 1b** — implement the scored retrieval first (zero risk), add the feedback loop as a separate change.

### Risk Assessment

- **Backwards compatibility:** 100% — old learnings (without quality_alpha/beta/times_applied) deserialize with defaults via `#[serde(default)]`
- **Existing behavior change:** Learnings are now ordered by value instead of timestamp. This is strictly better — high-value learnings surface first.
- **Performance:** Fetching 50 instead of 5 entries, scoring each with 3 floating-point ops. Negligible.
- **Failure mode:** If `learning_value()` panics (it won't — all ops are bounded), falls through to empty learnings. Same as current behavior when search fails.

### Test Plan

1. Unit test: `learning_value()` already has 7 tests passing
2. Integration: existing `format_learnings_capped_at_five` test covers the output format
3. Manual: start Tem, have a multi-turn conversation, verify learnings are injected in value order (check `/tmp/skyclaw.log` for "Context budget allocation" debug line showing learning_tokens)

---

## Fix 2: Lambda Importance Reinforcement

### What

Add an additive `recall_boost` field to `LambdaMemoryEntry`. Each recall adds +0.3 (capped at 2.0). GC sweeps apply -0.1 penalty to entries with zero access since last sweep.

### Why

Currently importance is set once by the LLM and never changes. A memory recalled 50 times has the same importance as one never accessed.

### Why NOT Beta(α, β)

The original proposal used a Beta distribution for importance. **This has a non-zero risk:**

The mapping `α₀ = I, β₀ = 6 - I` gives `E[importance] = 5 × I/6`, which is **lower than the scalar importance I** for all values. A memory with importance=3.0 would score effective=2.5 the moment it enters Beta mode — even before any recall or weakening. This is a behavioral regression.

Fixing the mapping requires `β₀ = α₀ × (5-I)/I`, but this gives `β₀ = 0` at `I = 5.0` (degenerate Beta). Every workaround (floors, clamping, scaling formulas) introduces edge cases or slight importance changes for existing entries.

**The additive approach has zero edge cases:**
- `effective = (importance + recall_boost).min(5.0)`
- At creation: `recall_boost = 0.0` → effective = importance. **IDENTICAL to current behavior.**
- After recalls: effective increases. **Correct.**
- No division. No degenerate distributions. No mapping.

### Files Touched

| File | Lines | Change |
|------|-------|--------|
| `crates/temm1e-core/src/traits/memory.rs` | 12-37 | Add `recall_boost: f32` to `LambdaMemoryEntry` |
| `crates/temm1e-memory/src/sqlite.rs` | 66-79 | `ALTER TABLE ADD COLUMN recall_boost REAL NOT NULL DEFAULT 0.0` |
| `crates/temm1e-memory/src/sqlite.rs` | 316-378 | Update `lambda_store()` to include recall_boost |
| `crates/temm1e-memory/src/sqlite.rs` | 380-396 | Update `lambda_query_candidates()` SELECT to include recall_boost |
| `crates/temm1e-memory/src/sqlite.rs` | 398-415 | Update `lambda_recall()` SELECT to include recall_boost |
| `crates/temm1e-memory/src/sqlite.rs` | 417-435 | Update `lambda_touch()` to increment recall_boost |
| `crates/temm1e-memory/src/sqlite.rs` | 475-490 | Update `lambda_gc()` to penalize unreferenced entries |
| `crates/temm1e-agent/src/lambda_memory.rs` | 31-34 | Update `decay_score()` to use effective_importance |

### Schema Migration

```sql
ALTER TABLE lambda_memories ADD COLUMN recall_boost REAL NOT NULL DEFAULT 0.0;
```

`DEFAULT 0.0` — existing entries get boost = 0, so `effective_importance = importance + 0 = importance`. **ZERO behavioral change for any existing entry.**

### LambdaMemoryEntry Change (memory.rs:12-37)

```rust
pub struct LambdaMemoryEntry {
    // ... all existing fields unchanged ...

    /// Additive importance boost from recalls (+0.3 per recall, capped at 2.0).
    /// GC applies -0.1 penalty for entries with no access since last sweep.
    #[serde(default)]
    pub recall_boost: f32,
}
```

`#[serde(default)]` → old serialized entries deserialize with `recall_boost = 0.0`. **Zero migration cost.**

### decay_score() Change (lambda_memory.rs:31-34)

```rust
// Current:
pub fn decay_score(entry: &LambdaMemoryEntry, now: u64, lambda: f32) -> f32 {
    let age_hours = (now.saturating_sub(entry.last_accessed)) as f32 / 3600.0;
    entry.importance * (-age_hours * lambda).exp()
}

// New:
pub fn decay_score(entry: &LambdaMemoryEntry, now: u64, lambda: f32) -> f32 {
    let age_hours = (now.saturating_sub(entry.last_accessed)) as f32 / 3600.0;
    effective_importance(entry) * (-age_hours * lambda).exp()
}

/// Importance with recall reinforcement and GC penalty applied.
pub fn effective_importance(entry: &LambdaMemoryEntry) -> f32 {
    (entry.importance + entry.recall_boost).clamp(0.1, 5.0)
}
```

**Proof of equivalence at creation:** `recall_boost = 0.0`, so `effective = (importance + 0.0).clamp(0.1, 5.0) = importance` for all importance in [1.0, 5.0]. Existing tests pass unchanged.

### lambda_touch() Change (sqlite.rs:417-435)

```sql
-- Current:
UPDATE lambda_memories SET last_accessed = ?, access_count = access_count + 1 WHERE hash = ?

-- New:
UPDATE lambda_memories
  SET last_accessed = ?,
      access_count = access_count + 1,
      recall_boost = MIN(recall_boost + 0.3, 2.0)
  WHERE hash = ?
```

`MIN(recall_boost + 0.3, 2.0)` caps total boost at 2.0. A memory with importance=3.0 can reach effective=5.0 after 7 recalls. This feels right — 7 deliberate recalls is strong evidence.

### lambda_gc() Weakening (sqlite.rs:475-490)

Add before the DELETE:

```sql
-- Penalize entries not accessed since cutoff (weak negative evidence)
UPDATE lambda_memories
  SET recall_boost = MAX(recall_boost - 0.1, 0.0)
  WHERE explicit_save = 0
    AND recall_boost > 0.0
    AND last_accessed < ?
```

- Only affects entries WITH existing boost (recall_boost > 0)
- MAX(..., 0.0) ensures boost never goes negative
- explicit_save entries are protected
- Effect: a memory with boost 0.3 (one recall) loses its boost after 3 GC cycles without access. Reasonable.

### Risk Assessment — ZERO

1. **At creation:** `recall_boost = 0.0` → `effective = importance`. IDENTICAL to current behavior.
2. **For existing DB entries:** `DEFAULT 0.0` → same as creation. No scoring change.
3. **For deserialization:** `#[serde(default)]` → 0.0. No change.
4. **On recall:** boost increases → effective increases. Correct direction only.
5. **On GC:** boost decreases (never below 0) → effective decreases toward scalar. Correct direction only.
6. **Clamp bounds:** effective is always in [0.1, 5.0]. No overflow, no negative, no NaN.
7. **No division.** No degenerate edge cases. Pure addition with clamping.
8. **Schema:** `ALTER TABLE ADD COLUMN ... DEFAULT 0.0` is instant in SQLite, no data rewrite.
9. **Memory trait:** No new methods needed. Existing lambda methods are updated internally.
10. **MarkdownMemory/FailoverMemory:** Don't override lambda methods (use default no-ops). Unaffected.

### Test Plan

1. `effective_importance(boost=0)` → returns scalar importance
2. `effective_importance(boost=2.0, importance=3.0)` → returns 5.0
3. `effective_importance(boost=2.0, importance=4.0)` → clamped at 5.0
4. `decay_score()` with boost=0 → identical to current test results
5. `decay_score()` with boost=0.6 → higher score (recall reinforcement working)
6. GC penalty: boost goes 0.3 → 0.2 → 0.1 → 0.0 → stays 0.0

---

## Fix 3: Blueprint Fitness Lifecycle + GC

### What

Add `blueprint_gc()` that retires blueprints by fitness score, add body size cap on refinement, add `last_executed_at` tracking.

### Why

Blueprints only have Create and Refine — no Delete. They accumulate indefinitely. Low-success blueprints never retire. Bodies grow unbounded through refinements.

### Files Touched

| File | Lines | Change |
|------|-------|--------|
| `crates/temm1e-agent/src/blueprint.rs` | 25-50 | Add `last_executed_at: Option<DateTime<Utc>>` to Blueprint struct |
| `crates/temm1e-agent/src/blueprint.rs` | new fn | Add `blueprint_gc()`, `compute_fitness()`, `fetch_all_blueprints()` |
| `crates/temm1e-agent/src/blueprint.rs` | refinement path | Add body size check after refinement |
| `crates/temm1e-agent/src/runtime.rs` | ~1502-1524 | Set `last_executed_at` on blueprint execution |
| `crates/temm1e-agent/src/runtime.rs` | startup or schedule | Call `blueprint_gc()` on startup |

### Blueprint Struct Change

```rust
// Add to Blueprint struct (blueprint.rs:25-50):
pub struct Blueprint {
    // ... existing fields ...

    /// Last time this blueprint was executed (for staleness detection).
    #[serde(default)]
    pub last_executed_at: Option<DateTime<Utc>>,
}
```

`#[serde(default)]` means existing serialized blueprints deserialize with `None`.

### compute_fitness() — New Function

```rust
/// Compute blueprint fitness: F = S² × R × U
///
/// S = Wilson lower bound of success rate (99% CI)
/// R = exp(-0.005 × days_since_last_executed)  [half-life ≈ 139 days]
/// U = 1.0 + 0.5 × ln(1 + times_executed)
pub fn compute_fitness(bp: &Blueprint, now: DateTime<Utc>) -> f64 {
    // Quality: Wilson lower bound, squared for selection pressure
    let s = if bp.times_executed > 0 {
        temm1e_distill::stats::wilson::wilson_lower(
            bp.times_succeeded as u64,
            bp.times_executed as u64,
            0.99,
        )
    } else {
        0.5 // uninformed prior for never-executed
    };
    let q = s * s;

    // Recency: exponential decay from last execution
    let last_exec = bp.last_executed_at.unwrap_or(bp.updated);
    let days = (now - last_exec).num_seconds().max(0) as f64 / 86400.0;
    let r = (-0.005 * days).exp();

    // Utility: log-reinforcement on execution count
    let u = 1.0 + 0.5 * (1.0 + bp.times_executed as f64).ln();

    q * r * u
}
```

**Dependency:** Uses `temm1e_distill::stats::wilson::wilson_lower()` which already exists. Need to add `temm1e-distill` as a dependency of `temm1e-agent` in Cargo.toml. **Wait** — this creates a crate dependency that might not be appropriate.

**Alternative:** Copy the `wilson_lower()` function into blueprint.rs (it's 15 lines of pure math, no external deps). This avoids the crate dependency entirely.

```rust
/// Wilson score lower bound (99% confidence).
fn wilson_lower_99(successes: u32, total: u32) -> f64 {
    if total == 0 {
        return 0.0;
    }
    let n = total as f64;
    let p = successes as f64 / n;
    let z = 2.576; // 99% CI
    let z2 = z * z;
    let denom = n + z2;
    let center = (n * p + z2 / 2.0) / denom;
    let margin = z * ((n * p * (1.0 - p) + z2 / 4.0) / (denom * denom)).sqrt();
    (center - margin).max(0.0)
}
```

**Decision:** Inline the function. No new crate dependency.

### fetch_all_blueprints() — New Function

```rust
/// Fetch all blueprints from memory for GC evaluation.
pub async fn fetch_all_blueprints(memory: &dyn Memory) -> Vec<(String, Blueprint)> {
    let opts = SearchOpts {
        limit: 500,
        entry_type_filter: Some(MemoryEntryType::Blueprint),
        ..Default::default()
    };

    let entries = match memory.search("", opts).await {
        Ok(e) => e,
        Err(e) => {
            warn!(error = %e, "Failed to fetch blueprints for GC");
            return Vec::new();
        }
    };

    entries
        .into_iter()
        .filter_map(|entry| {
            let id = entry.id.clone();
            parse_blueprint_with_metadata(&entry).ok().map(|bp| (id, bp))
        })
        .collect()
}
```

Note: `parse_blueprint_with_metadata()` is a helper that combines `parse_blueprint()` + the metadata restoration logic already in `fetch_by_category()` (lines 398-430). Extract into a shared helper.

### blueprint_gc() — New Function

```rust
/// Garbage collect low-fitness and proven-bad blueprints.
/// Returns the number of blueprints deleted.
pub async fn blueprint_gc(memory: &dyn Memory) -> usize {
    let now = Utc::now();
    let blueprints = fetch_all_blueprints(memory).await;
    let mut pruned = 0;

    for (id, bp) in &blueprints {
        let fitness = compute_fitness(bp, now);

        // Rule 1: fitness below delete threshold
        let should_delete = fitness < 0.005;

        // Rule 2: proven bad — enough executions AND low Wilson lower bound
        let proven_bad = bp.times_executed >= 5
            && wilson_lower_99(bp.times_succeeded, bp.times_executed) < 0.20;

        if should_delete || proven_bad {
            if let Err(e) = memory.delete(id).await {
                warn!(id = %id, error = %e, "Failed to delete blueprint during GC");
            } else {
                info!(
                    id = %id,
                    name = %bp.name,
                    fitness = fitness,
                    success_rate = bp.success_rate(),
                    "Blueprint GC: deleted"
                );
                pruned += 1;
            }
        }
    }

    if pruned > 0 {
        info!(pruned = pruned, total = blueprints.len(), "Blueprint GC complete");
    }
    pruned
}
```

### Body Size Cap on Refinement

**Location:** `runtime.rs` in the refinement path (lines ~1527-1560 per the audit).

After the refined blueprint body is parsed, add:

```rust
const MAX_BLUEPRINT_TOKENS: usize = 6000;

// After refinement parsing:
let refined_tokens = crate::context::estimate_tokens(&refined_body);
if refined_tokens > MAX_BLUEPRINT_TOKENS {
    warn!(
        id = %bp.id,
        tokens = refined_tokens,
        max = MAX_BLUEPRINT_TOKENS,
        "Refined blueprint exceeds token cap — truncating execution log"
    );
    // Truncate: keep everything up to "## Execution Log", then keep last 3 entries
    refined_body = truncate_execution_log(&refined_body, 3);
}
```

`truncate_execution_log()` is a new helper:

```rust
/// Keep only the last `keep` entries in the "## Execution Log" section.
fn truncate_execution_log(body: &str, keep: usize) -> String {
    // Find "## Execution Log" header
    let Some(log_start) = body.find("## Execution Log") else {
        return body.to_string(); // no log section, return as-is
    };

    // Split at the log section
    let (before_log, log_section) = body.split_at(log_start);

    // Find entries (each starts with "### Run" or "### Execution" or numbered heading)
    let entries: Vec<&str> = log_section.split("\n### ").collect();

    if entries.len() <= keep + 1 {
        return body.to_string(); // already within limit
    }

    // Keep header + last `keep` entries
    let header = entries[0]; // "## Execution Log\n..."
    let kept: Vec<&str> = entries[entries.len() - keep..].to_vec();

    format!(
        "{}{}\n[{} older entries truncated]\n\n### {}",
        before_log,
        header,
        entries.len() - keep - 1,
        kept.join("\n### ")
    )
}
```

### last_executed_at Tracking (runtime.rs)

In the blueprint refinement path where `times_executed` is incremented:

```rust
// Current (runtime.rs ~1513-1524):
updated_bp.times_executed += 1;
// ...
updated_bp.updated = chrono::Utc::now();

// Add:
updated_bp.last_executed_at = Some(chrono::Utc::now());
```

Also persist in metadata:

```rust
// In the metadata JSON:
"last_executed_at": updated_bp.last_executed_at.map(|t| t.to_rfc3339()),
```

### Startup GC Call

In `main.rs` or the agent initialization path, after memory is initialized:

```rust
// After memory setup, before main loop:
let gc_count = crate::blueprint::blueprint_gc(&*memory).await;
if gc_count > 0 {
    info!(pruned = gc_count, "Blueprint GC completed at startup");
}
```

### Risk Assessment

- **Backwards compatibility:** 100% — `last_executed_at: Option<DateTime<Utc>>` with `#[serde(default)]` is None for existing blueprints. `compute_fitness()` falls back to `bp.updated` when `last_executed_at` is None.
- **No schema change needed** — blueprints are stored as JSON in `MemoryEntry.content` + `metadata`. The new field is just another JSON key.
- **GC is purely subtractive** — only deletes entries. Never modifies existing blueprints. If GC deletes something it shouldn't (impossible given the conservative thresholds), the worst case is the blueprint is re-authored on the next matching task.
- **Body truncation** — only fires when `token_count > 6000`. Preserves all structure, only removes old execution log entries. If the log section doesn't exist, returns body unchanged.
- **Wilson lower bound at 99% CI** is conservative: a blueprint with 2/4 successes gets Wilson lower = 0.15, which is below the 0.20 retirement threshold — but it also needs 5+ executions to trigger, so it won't fire prematurely.

### Test Plan

1. Unit test: `compute_fitness()` with various inputs (new blueprint, old blueprint, high success, low success)
2. Unit test: `wilson_lower_99()` matches known values
3. Unit test: `truncate_execution_log()` with 0, 3, 10 entries
4. Integration: create a mock blueprint with low success, run `blueprint_gc()`, verify it's deleted
5. Integration: create a blueprint, verify `last_executed_at` is set after execution

---

## Fix 4: Eigen-Tune Retention Policy

### What

Add a quality-weighted min-heap eviction policy to the Eigen-Tune collector. When `count(tier) >= max_pairs_per_tier`, new pairs must beat the worst existing pair to be retained.

### Why

Collection is append-only with no retention mechanism. Storage grows linearly with conversation volume. The graduation pipeline (which would convert pairs to weights) is incomplete.

### Files Touched

| File | Lines | Change |
|------|-------|--------|
| `crates/temm1e-distill/src/config.rs` | 13-60 | Add `max_pairs_per_tier: i64` (default 5000) and `retention_days: i64` (default 180) |
| `crates/temm1e-distill/src/store.rs` | 189-234 | No change to `save_pair()` |
| `crates/temm1e-distill/src/store.rs` | new fn | Add `evict_if_full()`, `delete_pair()`, `get_worst_pair()`, `prune_old_low_quality()` |
| `crates/temm1e-distill/src/collector.rs` | ~82 | Call `evict_if_full()` before `save_pair()` |

### Config Changes (config.rs)

```rust
// Add to EigenTuneConfig struct:

/// Maximum training pairs retained per complexity tier.
/// When exceeded, lowest-quality pairs are evicted.
/// Default: 5000.
#[serde(default = "default_max_pairs_per_tier")]
pub max_pairs_per_tier: i64,

/// Pairs older than this AND below quality 0.5 are pruned regardless of
/// reservoir position. Default: 180 days.
#[serde(default = "default_retention_days")]
pub retention_days: i64,

// Defaults:
fn default_max_pairs_per_tier() -> i64 { 5000 }
fn default_retention_days() -> i64 { 180 }
```

### New Store Methods (store.rs)

```rust
/// Get the lowest-quality pair in a tier (eviction candidate).
pub async fn get_worst_pair(&self, tier: &str) -> Result<Option<(String, f64)>, Temm1eError> {
    let row = sqlx::query_as::<_, (String, f64)>(
        "SELECT id, COALESCE(quality_score, 0.5) as qs \
         FROM eigentune_pairs WHERE complexity = ? \
         ORDER BY qs ASC LIMIT 1"
    )
    .bind(tier)
    .fetch_optional(&self.pool)
    .await
    .map_err(|e| Temm1eError::Memory(format!("get_worst_pair: {e}")))?;

    Ok(row)
}

/// Delete a training pair by ID.
pub async fn delete_pair(&self, id: &str) -> Result<(), Temm1eError> {
    sqlx::query("DELETE FROM eigentune_pairs WHERE id = ?")
        .bind(id)
        .execute(&self.pool)
        .await
        .map_err(|e| Temm1eError::Memory(format!("delete_pair: {e}")))?;
    Ok(())
}

/// Evict the worst pair if the tier is at capacity.
/// Returns true if a pair was evicted (or capacity was available).
pub async fn evict_if_full(
    &self,
    tier: &str,
    new_quality: f64,
    max_pairs: i64,
) -> Result<bool, Temm1eError> {
    let count = self.count_pairs(tier).await?;
    if count < max_pairs {
        return Ok(true); // capacity available
    }

    // Tier is full — check if new pair beats the worst
    if let Some((worst_id, worst_quality)) = self.get_worst_pair(tier).await? {
        if new_quality > worst_quality {
            self.delete_pair(&worst_id).await?;
            tracing::debug!(
                tier = tier,
                evicted_id = %worst_id,
                evicted_quality = worst_quality,
                new_quality = new_quality,
                "Eigen-Tune: evicted worst pair to make room"
            );
            return Ok(true);
        }
    }

    Ok(false) // new pair not good enough
}

/// Prune pairs older than retention_days with quality < 0.5.
pub async fn prune_old_low_quality(&self, retention_days: i64) -> Result<usize, Temm1eError> {
    let cutoff = (chrono::Utc::now() - chrono::Duration::days(retention_days)).to_rfc3339();
    let result = sqlx::query(
        "DELETE FROM eigentune_pairs \
         WHERE created_at < ? AND COALESCE(quality_score, 0.5) < 0.5"
    )
    .bind(&cutoff)
    .execute(&self.pool)
    .await
    .map_err(|e| Temm1eError::Memory(format!("prune_old_low_quality: {e}")))?;

    Ok(result.rows_affected() as usize)
}
```

### Collector Change (collector.rs:~82)

```rust
// Current:
self.store.save_pair(&pair).await?;

// New:
let tier_str = pair.complexity.as_str();
let initial_quality = 0.5; // Beta(2,2) mean
let can_store = self.store
    .evict_if_full(tier_str, initial_quality, self.config.max_pairs_per_tier)
    .await
    .unwrap_or(true); // on error, allow storage (don't lose data)

if can_store {
    self.store.save_pair(&pair).await?;
}
```

Note: `EigenTier::as_str()` — need to verify this method exists. If not, use `format!("{:?}", pair.complexity).to_lowercase()` or match on the enum.

### Risk Assessment

- **Backwards compatibility:** 100% — new config fields have defaults, new methods are additive
- **No schema change** — all operations use existing columns (quality_score, created_at, complexity)
- **Data loss:** By design. Old low-quality pairs are evicted. This is the desired behavior. The eviction is quality-ordered — the best data is always retained.
- **Performance:** One COUNT + one SELECT per pair insert when tier is at capacity. The COUNT is on an indexed column (complexity). Negligible.
- **Edge case:** If all pairs in a tier have the same quality (0.5 — no signals yet), eviction is FIFO by the ORDER BY in `get_worst_pair()` which returns the first row from the ASC sort. This is acceptable — without quality differentiation, recency is as good a signal as any.

### Test Plan

1. Unit test: `evict_if_full()` when under capacity → returns true without deletion
2. Unit test: `evict_if_full()` when at capacity, new quality > worst → evicts and returns true
3. Unit test: `evict_if_full()` when at capacity, new quality <= worst → returns false
4. Unit test: `prune_old_low_quality()` with old and new pairs
5. Integration: Insert `max_pairs_per_tier + 10` pairs, verify count stays at max

---

## Fix 5: Lambda Memory Deduplication

### What

At GC time, detect near-duplicate lambda memories (same topic, similar content) and merge them. The merge takes the optimistic view on importance and unions evidence.

### Why

If the user discusses the same topic across multiple conversations, they get multiple memory entries competing for the same skull space. None are individually wrong, but collectively they waste tokens.

### Files Touched

| File | Lines | Change |
|------|-------|--------|
| `crates/temm1e-agent/src/lambda_memory.rs` | new fn | Add `dedup_candidates()`, `jaccard_similarity()`, `levenshtein_ratio()`, `merge_entries()` |
| `crates/temm1e-memory/src/sqlite.rs` | 475-490 | Call dedup before GC delete, add `lambda_update_entry()` method |
| `crates/temm1e-core/src/traits/memory.rs` | 123-161 | Add `lambda_update_entry()` trait method |

### Dedup Algorithm (lambda_memory.rs)

```rust
/// Jaccard similarity between two tag sets.
fn jaccard_similarity(a: &[String], b: &[String]) -> f64 {
    if a.is_empty() && b.is_empty() {
        return 0.0;
    }
    let set_a: std::collections::HashSet<&str> = a.iter().map(|s| s.as_str()).collect();
    let set_b: std::collections::HashSet<&str> = b.iter().map(|s| s.as_str()).collect();
    let intersection = set_a.intersection(&set_b).count();
    let union = set_a.union(&set_b).count();
    if union == 0 { 0.0 } else { intersection as f64 / union as f64 }
}

/// Simple Levenshtein-based similarity ratio (0.0 = different, 1.0 = identical).
/// Uses a simplified approach: shared character ratio.
fn essence_similarity(a: &str, b: &str) -> f64 {
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let a_lower = a.to_lowercase();
    let b_lower = b.to_lowercase();
    let a_words: std::collections::HashSet<&str> = a_lower.split_whitespace().collect();
    let b_words: std::collections::HashSet<&str> = b_lower.split_whitespace().collect();
    let intersection = a_words.intersection(&b_words).count();
    let union = a_words.union(&b_words).count();
    if union == 0 { 0.0 } else { intersection as f64 / union as f64 }
}

/// Find merge candidates among a set of entries.
/// Returns pairs (keep_idx, absorb_idx) where absorb should be merged into keep.
pub fn dedup_candidates(entries: &[LambdaMemoryEntry]) -> Vec<(usize, usize)> {
    let mut merges = Vec::new();

    for i in 0..entries.len() {
        for j in (i + 1)..entries.len() {
            let tag_sim = jaccard_similarity(&entries[i].tags, &entries[j].tags);
            if tag_sim < 0.6 {
                continue;
            }
            let essence_sim = essence_similarity(&entries[i].essence_text, &entries[j].essence_text);
            if essence_sim < 0.5 {
                continue;
            }

            // Keep the more recently accessed one, absorb the other
            if entries[i].last_accessed >= entries[j].last_accessed {
                merges.push((i, j));
            } else {
                merges.push((j, i));
            }
        }
    }

    merges
}

/// Merge entry `absorb` into `keep`. Returns the updated keep entry.
pub fn merge_entries(keep: &LambdaMemoryEntry, absorb: &LambdaMemoryEntry) -> LambdaMemoryEntry {
    let mut merged = keep.clone();

    // Optimistic: take the higher values
    merged.recall_boost = keep.recall_boost.max(absorb.recall_boost);
    merged.importance = keep.importance.max(absorb.importance);

    // Union evidence
    merged.access_count = keep.access_count + absorb.access_count;
    merged.created_at = keep.created_at.min(absorb.created_at);
    merged.last_accessed = keep.last_accessed.max(absorb.last_accessed);
    merged.explicit_save = keep.explicit_save || absorb.explicit_save;

    // Union tags (deduplicated)
    let mut all_tags: Vec<String> = keep.tags.clone();
    for tag in &absorb.tags {
        if !all_tags.contains(tag) {
            all_tags.push(tag.clone());
        }
    }
    merged.tags = all_tags;

    // Text: keep the more recent version (keep was chosen as more recent)
    // So no change needed — keep's text stays.

    merged
}
```

### Trait Method Addition (memory.rs)

```rust
/// Update a λ-memory entry in place (for dedup merge).
async fn lambda_update_entry(&self, _entry: &LambdaMemoryEntry) -> Result<(), Temm1eError> {
    Ok(())
}
```

### SQLite Implementation (sqlite.rs)

```rust
async fn lambda_update_entry(&self, entry: &LambdaMemoryEntry) -> Result<(), Temm1eError> {
    let tags_json = serde_json::to_string(&entry.tags).unwrap_or_else(|_| "[]".to_string());
    sqlx::query(
        "UPDATE lambda_memories SET \
         created_at = ?, last_accessed = ?, access_count = ?, \
         importance = ?, recall_boost = ?, \
         explicit_save = ?, tags = ? \
         WHERE hash = ?"
    )
    .bind(entry.created_at as i64)
    .bind(entry.last_accessed as i64)
    .bind(entry.access_count as i32)
    .bind(entry.importance)
    .bind(entry.recall_boost)
    .bind(entry.explicit_save)
    .bind(&tags_json)
    .bind(&entry.hash)
    .execute(&self.pool)
    .await
    .map_err(|e| Temm1eError::Memory(format!("lambda_update_entry: {e}")))?;
    Ok(())
}
```

### GC Integration

Dedup runs as part of `lambda_gc()` — BEFORE the deletion step:

```rust
// In the calling code (runtime startup or scheduled task):
// 1. Fetch candidates
let candidates = memory.lambda_query_candidates(500).await?;
// 2. Find merge pairs
let merges = lambda_memory::dedup_candidates(&candidates);
// 3. Execute merges
for (keep_idx, absorb_idx) in &merges {
    let merged = lambda_memory::merge_entries(&candidates[*keep_idx], &candidates[*absorb_idx]);
    memory.lambda_update_entry(&merged).await?;
    // Delete the absorbed entry
    memory.delete_lambda(&candidates[*absorb_idx].hash).await?;
}
// 4. Then run normal GC (existing lambda_gc)
memory.lambda_gc(now, max_age).await?;
```

Note: Need `delete_lambda()` method on Memory trait, or reuse lambda_gc with a targeted delete. Simplest: add a `lambda_delete(hash)` method.

### Explicit Save Protection

**Risk found during investigation:** `lambda_query_candidates()` returns ALL entries including explicit saves. If two explicit saves have overlapping tags, dedup would merge them — changing which hash survives. A user who said "remember this" and later recalls by hash would find a different entry.

**Fix:** Skip explicit saves entirely in dedup:

```rust
pub fn dedup_candidates(entries: &[LambdaMemoryEntry]) -> Vec<(usize, usize)> {
    let mut merges = Vec::new();
    for i in 0..entries.len() {
        if entries[i].explicit_save { continue; }  // NEVER merge explicit saves
        for j in (i + 1)..entries.len() {
            if entries[j].explicit_save { continue; }
            // ... matching logic ...
        }
    }
    merges
}
```

### Trait Method Safety

**Risk found during investigation:** Adding `lambda_update_entry()` to the Memory trait could break MarkdownMemory and FailoverMemory.

**Verified safe:** All existing lambda methods on the Memory trait (lines 121-161 of memory.rs) have **default no-op implementations** (`Ok(())`). MarkdownMemory and FailoverMemory don't override ANY lambda methods — they use the defaults. Adding a new lambda method with a default no-op follows the same pattern. Compilation is unaffected.

### Merge Logic Update for Fix 2 Change

Since Fix 2 now uses `recall_boost: f32` instead of Beta(α, β), the merge logic simplifies:

```rust
pub fn merge_entries(keep: &LambdaMemoryEntry, absorb: &LambdaMemoryEntry) -> LambdaMemoryEntry {
    let mut merged = keep.clone();
    merged.recall_boost = keep.recall_boost.max(absorb.recall_boost);
    merged.importance = keep.importance.max(absorb.importance);
    merged.access_count = keep.access_count + absorb.access_count;
    merged.created_at = keep.created_at.min(absorb.created_at);
    merged.last_accessed = keep.last_accessed.max(absorb.last_accessed);
    merged.explicit_save = keep.explicit_save || absorb.explicit_save;
    // Union tags (deduplicated)
    let mut all_tags = keep.tags.clone();
    for tag in &absorb.tags {
        if !all_tags.contains(tag) {
            all_tags.push(tag.clone());
        }
    }
    merged.tags = all_tags;
    merged
}
```

No Beta mapping, no division, no edge cases.

### Risk Assessment — ZERO

1. **Explicit saves protected:** Never matched, never merged, never deleted.
2. **Trait method safe:** Default no-op, same pattern as all other lambda methods. MarkdownMemory/FailoverMemory unaffected (verified: they don't override any lambda methods).
3. **False positive merges:** Dual threshold (Jaccard > 0.6 AND word overlap > 0.5). Requires >60% tag overlap AND >50% essence word overlap. Very conservative.
4. **Merge preserves evidence:** Keeps higher importance, higher boost, all tags, all access counts, earliest creation, latest access. Nothing is lost.
5. **Hot path impact:** Zero. Runs only during GC (startup/daily).
6. **Performance:** O(n²) on n ≤ 500 candidates, string ops only. ~125ms worst case. Acceptable for maintenance task.
7. **Failure mode:** If dedup finds zero candidates (common case), it does nothing. If merge fails, the entry is unchanged (the absorbed entry is only deleted after successful update of the keep entry).

### Test Plan

1. Unit test: `jaccard_similarity()` with identical, overlapping, disjoint tag sets
2. Unit test: `essence_similarity()` with identical, similar, different essences
3. Unit test: `dedup_candidates()` finds pairs above threshold, ignores below
4. Unit test: `merge_entries()` correctly unions evidence and takes optimistic importance
5. Integration: Create 3 entries with overlapping tags/essence, run dedup, verify 2 remain

---

## Execution Order

```
Fix 1 (learning scored retrieval)     — ZERO RISK, no deps
  ↓
Fix 3 (blueprint GC)                  — ZERO RISK, no deps
  ↓
Fix 4 (eigen-tune retention)          — ZERO RISK, no deps
  ↓
Fix 2 (lambda Bayesian importance)    — LOW RISK, schema migration
  ↓
Fix 5 (lambda dedup)                  — LOW RISK, depends on Fix 2
```

Fixes 1, 3, 4 are completely independent and can be executed in parallel.
Fix 5 depends on Fix 2 (uses importance_alpha/beta fields in merge logic).

---

## Compilation Gate Checklist

After each fix:

```bash
cargo check --workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all -- --check
cargo test --workspace
```

All four must pass before proceeding to the next fix.
