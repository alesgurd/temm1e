# Tem Prowl Changelog

All changes on the `tem-browse` branch for Tem Prowl — web-native browsing with OTK authentication.

---

## 2026-03-20 — Initial Release

### New Modules (temm1e-tools)

- **browser_observation.rs** (446 LOC) — Layered observation architecture with three tiers: accessibility tree only (Tier 1), tree + DOM as Markdown (Tier 2), tree + screenshot (Tier 3). Deterministic `O(1)` tier selection via `TreeMetadata` analysis. Token cost scales `O(d * log c)` versus `O(d * c)` for screenshot-based agents.

- **browser_session.rs** (1,171 LOC) — Interactive OTK (One-Time Key) browser session for credential capture. Annotated screenshot flow with numbered interactive elements. Users click by number and type to fill fields. Credentials flow directly into the page via CDP `Input.insertText` — the LLM never sees them. Passwords wrapped in `Zeroizing<String>`. Captured sessions (cookies + localStorage) encrypted at rest via ChaCha20-Poly1305 vault.

- **browser_pool.rs** (361 LOC) — Browser context pool for swarm browsing. Single Chrome process with multiple isolated browser contexts. Lock-free atomic CAS bitset for slot allocation — zero contention between Hive workers. Default 4 contexts (~220-420 MB total). Configurable `max_size` based on available memory.

- **credential_scrub.rs** (268 LOC) — Credential scrubber applied to all browser observations post-credential-injection. Strips sensitive URL query parameters (token, key, secret, password, etc.), authorization headers, API key patterns, password field values, session/JWT tokens, and credit card numbers before they enter the LLM context.

- **prowl_blueprints.rs** (79 LOC) — Module root for web-specific Prowl blueprints.

- **prowl_blueprints/login_registry.rs** (281 LOC) — Login URL registry with 111 entries covering 100+ services across 10 categories: social media (Facebook, Instagram, Twitter/X, TikTok, LinkedIn, Reddit, etc.), messaging (Telegram, WhatsApp, Discord, Slack, etc.), Google services, Microsoft services, Apple, developer/code platforms (GitHub, GitLab, AWS, Vercel, etc.), shopping/e-commerce (Amazon, eBay, Shopify, etc.), entertainment/streaming (Netflix, Spotify, Twitch, etc.), productivity (Trello, Asana, Jira, Notion, etc.), finance (PayPal, Stripe, Coinbase, etc.), education (Coursera, Udemy, etc.), and AI tools (ChatGPT, Claude, etc.). Supports aliases (fb, gh, ig, x) and custom URL fallback.

### Web Blueprint Templates

- **prowl_blueprints/web_login.md** — Blueprint for authenticated login flows
- **prowl_blueprints/web_search.md** — Blueprint for web search and result extraction
- **prowl_blueprints/web_extract.md** — Blueprint for structured data extraction from web pages
- **prowl_blueprints/web_compare.md** — Blueprint for multi-site comparison tasks

### Modified Files

- **browser.rs** (+1,173 lines) — Extended `BrowserTool` with Prowl capabilities: `observe` action (accessibility tree extraction with layered observation), `authenticate` action (OTK session capture), `restore_web_session` action (session restore from vault), vault integration via `with_vault()` builder, accessibility tree formatting, JS-based element extraction (chromiumoxide 0.7 compatibility), annotated screenshot generation, and credential-scrubbed observation output.

- **lib.rs** (temm1e-tools) — Registered new modules behind `#[cfg(feature = "browser")]` feature gate. Added `BrowserPool` public re-export. Updated `create_tools()` to accept `vault: Option<Arc<dyn Vault>>` parameter for credential isolation.

- **main.rs** (+280 lines) — `/login <service>` command handler with login registry resolution. Per-chat `InteractiveBrowseSession` tracking. Active session interceptor routes user messages (numbers for clicks, text for typing, "done" to finish) through the interactive flow. System prompt additions: security instructions (never ask for passwords in chat), observe action guidance, session restore before login prompt.

- **Cargo.toml** (root) — Added `zeroize` to workspace dependencies.

- **Cargo.toml** (temm1e-tools) — Added `zeroize` dependency for credential memory safety.

- **agent_bridge.rs** (temm1e-tui) — Added vault parameter passthrough to `create_tools()`.

### Swarm Browsing (temm1e-hive)

- **pheromone.rs** — Added four browse-specific pheromone signal types: `BotDetected`, `SessionExpired`, `DataFound`, `RateLimit`. Enables emergent collective intelligence — the swarm learns which sites are hostile, which sessions have expired, and which results are available.

- **queen.rs** (+66 lines) — Web decomposition guide for the Queen (Alpha coordinator). `build_decomposition_prompt_with_tools()` detects browser availability and injects multi-site task decomposition rules: one subtask per domain, independent parallelism, aggregation task at the end.

- **types.rs** (+49 lines) — Added `BrowseTask` and `BrowseResult` types for swarm browser coordination. Browse-specific task metadata for Hive workers.

### Research & Documentation (tems_lab/)

- **TEM_PROWL_PAPER.md** — Full research paper: 5 contributions (layered observation, credential isolation, OTK session capture, resilience invariant, stigmergic web swarm). Formal token complexity bounds, security proofs, evaluation framework.

- **TEM_PROWL_RESEARCH.md** — Deep research report: industry landscape analysis, 10 framework comparisons, authentication approaches, token optimization strategies.

- **prowl/IMPLEMENTATION.md** — 6-phase implementation plan mapping paper architecture to code.
- **prowl/PHASE_0_FOUNDATION.md** through **prowl/PHASE_6_BENCHMARKS.md** — Per-phase implementation details.
- **prowl/BENCHMARK_REPORT.md** — Performance benchmarks.
- **prowl/EXPERIMENT_REPORT.md** — Experiment results.
- **prowl/MULTISTEP_BENCHMARK.md** — Multi-step task benchmarks.
- **prowl/OTK_DRY_RUN.md** — OTK protocol dry run results.
- **prowl/SYSTEM_PROMPT_ANALYSIS.md** — System prompt impact analysis.
- **prowl/TASK_MATRIX.md** — Web task coverage matrix.
- **prowl/UX_TESTING_REPORT.md** — User experience testing report.

### Summary

| Metric | Value |
|--------|-------|
| New modules | 6 |
| New LOC | ~4,490 (modules + modifications) |
| Blueprint templates | 4 |
| Login registry entries | 111 (100+ unique services) |
| Service categories | 10 |
| New pheromone signals | 4 |
| Research documents | 17 |
| Existing crates modified | 5 (temm1e-tools, temm1e-hive, temm1e-tui, root binary, root Cargo.toml) |
