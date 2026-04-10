# Tem-Code: Research Summary

**Full research paper:** `docs/TEM_CODE_RESEARCH.md`

This file summarizes key findings. The full paper has industry analysis of 8 agents, gap analysis, principles, and detailed designs.

## Core Thesis

The control plane around the model matters more than the model itself. Interface design can improve agent performance 2-3x without changing the model (SWE-agent). The most successful agents are the most disciplined, not the most complex (Claude Code's single-threaded loop, Mini-SWE-agent's 100 lines).

## Key Insights from Industry

1. **Edit tool design:** Exact string replacement > diffs > whole file rewrite. LLMs cannot count lines. Uniqueness constraint forces sufficient context. Read-before-edit prevents hallucinated edits.

2. **Dedicated tools > shell:** Output limiting (default 250 results), governance by tool name, permission caching, structured output with line numbers.

3. **Git safety as engineering discipline:** Branch-first, atomic commits, never force-push main, never amend without intent, stage by filename not `git add .`.

4. **Token efficiency:** Aider's tree-sitter repo map achieves 4.3-6.5% context utilization (best in class). Deferred tool loading saves 85% context. Output limiting is critical.

5. **Skull > Compaction:** Temm1e's Skull budget system already prevents context overflow via priority-based allocation + dropped summary injection + lambda memory. Compaction is redundant.

6. **AGI-first safety:** Self-governing guardrails (engineering discipline) not permission prompts (human-in-the-loop). Tem is autonomous — guardrails prevent catastrophic mistakes, not restrict autonomy.

7. **Dynamic history budgeting:** Hardcoded message counts (30-60) are anti-skull. Token-budgeted fractions scale with model size automatically.

## Harmony Audit: CLEAR

All 23 crates audited. Zero conflicts with proposed changes. See `HARMONY_AUDIT.md` for details.
