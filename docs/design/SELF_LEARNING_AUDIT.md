# Self-Learning Subsystem Audit

> Audit of all TEMM1E self-learning mechanisms against Pillar VII: Agentic Self-Learning.
>
> **Core principle under test:** *"Every self-learning loop must have a corresponding mechanism that either decays them, refines them, supersedes them, or graduates them into a form that does not consume context."*

---

## Audit Summary

| Subsystem | Produces Artifacts | Has Drain | Skull-Budgeted | Staleness Handling | Verdict |
|-----------|-------------------|-----------|----------------|-------------------|---------|
| **Lambda Memory** | Yes (memory entries) | Exponential decay + GC | Yes (dynamic residual) | Implicit via decay | COMPLIANT, two gaps |
| **Blueprints** | Yes (procedures) | Partial (refine only) | Yes (10% cap) | None | NON-COMPLIANT |
| **Cross-Task Learnings** | Yes (lessons) | None | Yes (5% cap) | None | NON-COMPLIANT |
| **Tem Anima** | Yes (user profiles) | Confidence decay + buffer caps | System constant (accepted) | Partial (confidence decay) | COMPLIANT |
| **Skills** | No (filesystem-sourced) | N/A (registry clears on reload) | No (tool-invoked, on-demand) | N/A | NOT A LEARNING LOOP |
| **Specialist Cores** | No (filesystem-sourced) | N/A (registry clears on reload) | Yes (per-invocation pruning) | N/A | NOT A LEARNING LOOP |
| **Eigen-Tune** | Yes (training pairs) | Designed but broken | N/A (not integrated into runtime) | N/A (not serving) | NON-COMPLIANT |

**Overall finding:** The system learns aggressively but prunes weakly. Of the four true self-learning loops (lambda, blueprints, learnings, eigen-tune), only lambda memory has a functioning drain. The skull budget is respected by the context builder but bypassed by four runtime-layer injections.

---

## Issue 1: Cross-Task Learnings Have Zero Drain

**Severity: HIGH**
**Files:** `crates/temm1e-agent/src/learning.rs`, `crates/temm1e-agent/src/context.rs:345-376`

### Problem

Learnings are the only self-learning subsystem with **no decay, no pruning, no supersession, and no staleness detection.** Once a `TaskLearning` is stored, it persists in SQLite forever.

- Storage: unlimited — every non-trivial task generates 1-3 learnings
- Retrieval: `memory.search("learning:", limit=5)` — picks the first 5 matches by relevance
- Injection: 5% of context budget, up to 5 learnings
- Contradiction: two learnings can directly contradict each other with no resolution
- Staleness: a learning from day 1 is treated identically to one from today

Over months of operation, the learnings table accumulates hundreds of entries. The search picks 5, but with no recency weighting, it may inject stale or contradicted lessons. Worse: a lesson like "shell+file: use `sed` for inline replacement" learned in a Linux context will mislead on a Windows task.

### Vision Conflict

Pillar VII: *"Stale artifacts are worse than no artifacts — they actively mislead. The learning loop inverts: the system gets worse by learning."*

This is exactly the failure mode. Learnings have no mechanism to prevent it.

### Additional Conflict: Rule-Based Extraction

Outcome determination uses keyword matching (`"successfully"`, `"failed"`, `"error"`) on the final assistant message. This violates the project's own `feedback_no_keyword_matching.md`: *"NEVER use keyword/string matching for semantic decisions; always LLM calls."*

The lesson text is assembled from tool names and error snippets — mechanical, not semantic. A learning like `[OK] shell+browser: Tools used: shell, browser_navigate, shell. Strategy rotation occurred.` carries little actionable insight.

### Proposed Fix

**1a. Add timestamp-weighted retrieval.** Modify the search to weight recent learnings higher:

```
score = relevance × recency_factor
recency_factor = exp(-age_days × 0.03)   // half-life ~23 days
```

**1b. Add supersession.** When a new learning is extracted for the same `task_type` and same `outcome`, check if an existing learning already covers it. If so, replace (or merge) rather than append.

**1c. Add garbage collection.** Learnings older than 90 days with no reinforcement should be pruned. Mirror lambda memory's GC policy: `last_accessed < 90d AND not explicitly saved`.

**1d. LLM-powered extraction.** Replace the keyword-based `determine_outcome()` and `generate_lesson()` with an inplace LLM call (same pattern as lambda memory's `worth_remembering` → `<memory>` block approach). The LLM sees the full conversation and produces a semantic lesson, not a tool-name concatenation.

---

## Issue 2: Blueprints Have No Deletion — The CRUD Loop is CR Only

**Severity: HIGH**
**Files:** `crates/temm1e-agent/src/blueprint.rs`, `crates/temm1e-agent/src/runtime.rs:1437-1567`

### Problem

Blueprints implement Create and Refine but not Delete or Retire. Two critical consequences:

**A. Unbounded body growth.** Each refinement can expand the blueprint body (adding execution logs, failure modes, phases). The authoring LLM has `max_tokens: 4096`, but refinements are not size-checked. After 10 refinements, a blueprint can exceed the 10% context budget and get permanently demoted to catalog-only mode — still occupying storage and search latency but never injected at full fidelity.

**B. No retirement of low-performing blueprints.** A blueprint with 20% success rate remains in the catalog indefinitely. The matching system will surface it if the semantic tags match, wasting context on a proven bad procedure.

**C. No deduplication.** Two blueprints with the same `task_signature` (e.g., `"shell+file"`) but different names coexist. The catalog shows both, splitting the LLM's attention.

### Vision Conflict

Pillar VII: *"A self-learning loop without a drain is a memory leak. And in an agent runtime designed for perpetual deployment, a memory leak is a countdown to failure."*

Blueprints are a memory leak. They accumulate without bound and never retire.

### Proposed Fix

**2a. Blueprint GC.** Add `blueprint_gc()` triggered daily (or on startup):

```
DELETE blueprints WHERE:
  - times_executed > 5 AND success_rate < 0.25   (proven bad)
  - updated < 90_days_ago AND times_executed == 0 (never used)
```

**2b. Body size cap on refinement.** After parsing the refined body, check `token_count`. If it exceeds a threshold (e.g., 6000 tokens), truncate the execution log to the last 3 entries and drop verbose failure mode descriptions.

**2c. Deduplication on creation.** Before creating a new blueprint, check if one with the same `task_signature` already exists. If so, merge execution metadata rather than creating a duplicate.

**2d. Staleness flag.** Add `last_executed_at: DateTime<Utc>` to the Blueprint struct. Blueprints not executed in 60+ days are deprioritized in matching (sorted below fresh ones).

---

## ~~Issue 3: Four Subsystems Inject Outside the Skull Budget~~ — DISMISSED

These injections (personality mode, anima profile, perpetuum temporal, consciousness whisper) are **system identity constants** — needed overhead for Tem's cognitive infrastructure. They are small, bounded by design, and not learning artifacts. Not a self-learning drain issue.

---

## Issue 4: Lambda Memory Importance Is Immutable

**Severity: MEDIUM**
**Files:** `crates/temm1e-agent/src/lambda_memory.rs`, `crates/temm1e-memory/src/sqlite.rs:417-435`

### Problem

When a lambda memory entry is created, the LLM assigns an importance score (1.0-5.0). This score **never changes.** Recall reheats the memory (resets `last_accessed`, increments `access_count`), but importance stays fixed.

This means a memory rated importance=2.0 at creation will always be at a fidelity disadvantage against a memory rated importance=4.0, even if the 2.0 memory has been recalled 50 times and the 4.0 memory has never been accessed.

The design doc acknowledges this as "Open Question #1": *"Should recall boost importance?"* — left unimplemented.

### Vision Conflict

Pillar VII describes importance as analogous to weights in traditional ML. In ML, weights are updated by gradient signals. Here, the "gradient signal" (recall frequency, user-triggered recall) exists but is not applied. The system has a feedback signal and throws it away.

### Proposed Fix

**4a. Importance reinforcement on recall.** When `lambda_touch()` fires:

```rust
// Reinforce importance by +0.3 per recall, capped at 5.0
entry.importance = (entry.importance + 0.3).min(5.0);
```

This is conservative — 10 recalls would take a 2.0 entry to 5.0, which feels right for something the user or agent has recalled 10 times.

**4b. Importance decay for never-accessed entries.** Entries with `access_count == 0` after 7 days could lose 0.5 importance. This implements "if nobody ever asked for this, it probably wasn't that important."

---

## Issue 5: Eigen-Tune Collection Is Unbounded

**Severity: MEDIUM**
**Files:** `crates/temm1e-distill/src/collector.rs`, `crates/temm1e-distill/src/store.rs`

### Problem

Eigen-Tune's collector captures every (request, response) pair. The design doc explicitly states this is "append-only, never deleted" (A4: Monotonic Data Growth). No retention policy exists.

For an agent processing 100 messages/day with average 2 LLM calls per message, that is 200 pairs/day, ~73,000 pairs/year. Each pair includes full `messages_json`, `system_prompt`, `tools_json`, `response_json` — conservatively 5-20KB per pair. That's 365MB-1.4GB/year of raw training data with no cleanup.

### Vision Conflict

Pillar VII: *"For every loop that produces artifacts, there must be a mechanism that either decays them, refines them, supersedes them, or graduates them."*

Eigen-Tune's design intention is graduation (artifacts → weights). But graduation is broken (`serving_run_id` never set, trainer/evaluator/curator not implemented). Until graduation works, collection is an unbounded append-only log with no drain.

### Proposed Fix

**5a. Retention policy.** Add configurable retention:

```toml
[eigentune]
max_pairs_per_tier = 10000
retention_days = 180
```

Pairs older than `retention_days` AND beyond `max_pairs_per_tier` are purged, lowest quality first.

**5b. Deduplication.** Near-duplicate pairs (same system_prompt + similar messages) should be detected and deduplicated. Keep the highest-quality instance.

**5c. Fix graduation pipeline.** The most important fix is to complete the graduation path so that artifacts actually convert to weights:
- Implement `curator.rs` (export ChatML JSONL)
- Implement `trainer.rs` (invoke Unsloth/MLX subprocess)
- Wire `graduation.rs` to set `serving_run_id` after successful training + evaluation
- This is the only subsystem designed to **eliminate its own artifacts** by converting them to model weights

---

## Issue 6: Skills and Cores Are Not Learning Systems

**Severity: LOW (observation, not bug)**
**Files:** `crates/temm1e-skills/src/lib.rs`, `crates/temm1e-cores/src/`

### Observation

The vision doc lists skills and cores as self-learning loops. In implementation, they are **static capability registries**:

- Skills: Markdown files loaded from filesystem. No runtime creation. No usage tracking. No refinement.
- Cores: Markdown files loaded from filesystem. `CoreStats` struct exists but is never populated. No feedback loop from execution outcomes to core prompt refinement.

These are currently **tools, not learning systems.** They expand what Tem *can do*, but they don't improve based on experience.

### Vision Alignment

Pillar VII claims: *"Skills — capability expansion — what I can do grows over time without code changes."*

This is aspirational, not implemented. Skills are admin-authored, not self-authored.

### Proposed Fix (Future)

**6a. Runtime skill authoring.** After completing a task that required a novel multi-tool procedure, the agent could author a new skill (similar to blueprint authoring but lighter — just a reference doc, not a full procedure).

**6b. Core prompt refinement.** When a core completes execution, record `CoreStats` (success rate, avg tool calls, avg duration). After N executions, offer to refine the core's system prompt via LLM — same pattern as blueprint refinement.

**6c. Usage tracking.** Add `last_invoked_at` and `invocation_count` to both skills and cores. This enables future staleness detection and popularity-based prioritization.

These are not urgent — skills and cores being static is fine for now. But they should be noted as future learning loops that will need drains when they become dynamic.

---

## Issue 7: Anima Confidence Decay Exists But Has No Graduation Path

**Severity: LOW**
**Files:** `crates/temm1e-anima/src/evaluator.rs:358-417`, `crates/temm1e-anima/src/storage.rs`

### Problem

Tem Anima has the most disciplined bounding of any subsystem:
- Hard buffer caps (30 facts, 50 observations, 100 eval logs)
- Confidence decay (5% per unobserved eval, zeros at <0.1)
- Fixed-size profile structure

However, high-confidence personality dimensions (e.g., "user is direct and technical") are re-derived from scratch every N turns via LLM evaluation. There's no mechanism to "graduate" stable personality traits to a cheaper representation — the same LLM call runs whether the profile has been stable for 5 evaluations or 500.

### Proposed Fix

**7a. Adaptive evaluation frequency is already implemented** (`n_next` grows logarithmically with stability). This is sufficient for now.

**7b. Future consideration:** Once a dimension reaches confidence >0.9 and has been stable for 20+ evaluations, it could be "graduated" to a frozen tier that skips re-evaluation until a behavioral shift is detected (delta >0.15 already triggers reset). This would reduce LLM calls for long-running deployments with consistent users.

---

## Cross-Cutting Issue: No Unified Artifact Lifecycle Manager

### Problem

Each subsystem implements its own drain (or doesn't). There is no unified mechanism that:
- Tracks total artifact count across all subsystems
- Enforces a global artifact budget
- Coordinates pruning when the system is under storage pressure
- Reports artifact health (growth rate, staleness distribution, drain effectiveness)

### Proposed Fix

**8a. Artifact health metrics.** Add to the observability layer:

```
temm1e_artifacts_total{subsystem="lambda_memory"} = 1247
temm1e_artifacts_total{subsystem="blueprints"} = 23
temm1e_artifacts_total{subsystem="learnings"} = 156
temm1e_artifacts_total{subsystem="eigentune_pairs"} = 8432
temm1e_artifacts_stale{subsystem="learnings"} = 89   // >90 days, never accessed
temm1e_artifact_growth_rate{subsystem="eigentune_pairs"} = 12.3/day
```

**8b. Global GC trigger.** A daily maintenance task that runs all subsystem GCs in sequence: lambda GC → blueprint GC → learning GC → eigentune retention. This ensures no subsystem is forgotten.

---

## Priority Matrix

| Issue | Severity | Effort | Impact on Perpetual Deployment | Recommended Phase |
|-------|----------|--------|-------------------------------|-------------------|
| **1. Learnings: zero drain** | HIGH | Medium | Stale learnings actively mislead over months | Phase 1 — **FIXED (V3 extraction + value function)** |
| **2. Blueprints: no deletion** | HIGH | Medium | Unbounded growth, wasted storage and search | Phase 1 |
| ~~**3. Skull budget bypass**~~ | ~~HIGH~~ | — | System constants, dismissed | — |
| **4. Lambda importance immutable** | MEDIUM | Low | Missed feedback signal, suboptimal retrieval | Phase 2 |
| **5. Eigen-Tune unbounded** | MEDIUM | Low (retention) / High (graduation) | Storage growth without value extraction | Phase 2 (retention) / Phase 3 (graduation) |
| **6. Skills/Cores not learning** | LOW | High | No self-improvement in capabilities | Phase 3+ |
| **7. Anima no graduation** | LOW | Low | Redundant LLM calls for stable profiles | Phase 3+ |
| **8. No unified lifecycle** | LOW | Medium | Fragmented maintenance, no global visibility | Phase 2 |
