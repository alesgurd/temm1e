<p align="center">
  <img src="assets/banner.png" alt="SkyClaw" width="100%">
</p>

<p align="center">
  Built with <a href="https://github.com/nagisanzenin/claude-code-production-grade-plugin">production-grade</a> — the Claude Code plugin for shipping real systems, not just code files.
</p>

<p align="center">
  <a href="https://github.com/nagisanzenin/skyclaw/stargazers"><img src="https://img.shields.io/github/stars/nagisanzenin/skyclaw?style=flat&color=gold&logo=github" alt="GitHub Stars"></a>
  <a href="https://discord.gg/3ux2c5xz"><img src="https://img.shields.io/badge/Discord-Join%20Community-5865F2?logo=discord&logoColor=white" alt="Discord"></a>
  <img src="https://img.shields.io/badge/License-MIT-yellow.svg" alt="MIT License">
  <img src="https://img.shields.io/badge/version-2.0.1-blue.svg" alt="Version">
  <img src="https://img.shields.io/badge/tests-1217-green.svg" alt="1217 tests">
  <img src="https://img.shields.io/badge/providers-7-red.svg" alt="7 providers">
</p>

# SkyClaw

Hyper-performance Rust agent runtime with extreme resilience and continuous self-learning. Deploys once, stays up forever. Learns from every task, remembers across sessions, self-heals through failures. **v2.0: LLM-powered chat/order classification — chat messages answered in 1 call, orders get instant acknowledgment while the pipeline works.** 46K lines, 1217 tests, zero warnings, zero panic paths.

## What It Does

SkyClaw is an autonomous AI agent that lives on your server and talks to you through messaging apps. It runs shell commands, browses the web, reads/writes files, fetches URLs, understands images, delegates sub-tasks, self-heals, and learns from its own mistakes — all controlled through natural conversation.

No web dashboards. No config files to edit. Deploy, paste your API key in Telegram, and go.

## AGENTIC CORE v2

SkyClaw's intelligence layer — 20 modules driving an autonomous execution cycle, now with **smart complexity classification** that understands what kind of task you're asking before it starts working.

### v2.1: LLM-Powered Chat/Order Classification

Every inbound message is classified by a **single fast LLM call** that serves dual purpose — classify AND respond:

```
Message arrives
    ↓
[LLM CLASSIFY] ─→ Chat  ──→ return response immediately (1 call total)
                → Order ──→ send acknowledgment instantly
                             ↓
                         [PIPELINE] ─→ runs until done
                                       (budget + time are the only limits)
```

| What you say | Category | What happens |
|-------------|----------|-------------|
| "What's the capital of France?" | Chat | LLM answers directly. Done. 1 call. |
| "Thanks!" / "Ok" | Chat | LLM responds naturally. Done. 1 call. |
| "Open YouTube and search for news" | Order (standard) | User sees instant ack, pipeline runs. |
| "Find a poem, translate it, export as DOCX" | Order (complex) | User sees instant ack, full pipeline with max context. |

**Key insight:** Chat messages never enter the tool loop — zero wasted tokens. Order messages get an instant acknowledgment so the user isn't staring at silence while the pipeline works. Multilingual — the LLM classifies and responds in the user's language. No artificial iteration caps — budget and time limits are the natural guardrails.

Fallback: if the LLM classify call fails (network error, parse failure), rule-based classification kicks in automatically.

### Agent Loop

```
ORDER ─→ THINK ─→ ACTION ─→ VERIFY ─┐
                                      │
          ┌───────────────────────────┘
          │
          ├─ DONE? ──→ yes ──→ LEARN ──→ END
          │
          └─ no ──→ THINK ─→ ACTION ─→ VERIFY ─→ ...
```

- **ORDER**: Inbound message decomposed into task graph
- **THINK**: Context assembly — system prompt, tool defs, memory, knowledge, past learnings (5% budget)
- **ACTION**: Tool execution — shell, browser, file ops, web fetch, git
- **VERIFY**: Self-correction engine checks output, triggers strategy rotation on repeated failures
- **DONE**: Measurable completion criteria, not assertions
- **LEARN**: `extract_learnings()` analyzes tools used, failures, outcomes → stores `TaskLearning` in memory → injected into future THINK steps

| Category | Modules |
|----------|---------|
| **Resilience** | Zero panic paths in production code, circuit breaker with exponential backoff, per-message panic recovery, dead worker respawn with message re-dispatch, send retry (3 attempts), channel reconnection, 5s DB timeout, graceful SIGTERM drain, lock poison recovery, conversation persistence |
| **Intelligence** | Task decomposition, self-correction, DONE criteria, cross-task learning, **complexity classification (v2)**, **prompt stratification (v2)** |
| **Self-Healing** | Watchdog, state recovery, health-aware heartbeat, memory failover |
| **Efficiency** | Output compression, system prompt optimization, tiered model routing, history pruning, **complexity-aware tool loop (v2)**, **execution profiles (v2)** |
| **Autonomy** | Parallel tool execution, agent-to-agent delegation, proactive task initiation, adaptive system prompt |
| **Multimodal** | Vision / image understanding (JPEG, PNG, GIF, WebP) |

## Key Metrics

| Metric | Value |
|--------|-------|
| **Lines of Rust** | 46,827 across 104 source files |
| **Tests** | 1,217 passing, 0 failures |
| **Clippy warnings** | 0 (CI gate: `-D warnings`) |
| **Workspace crates** | 13 + 1 binary |
| **Implemented features** | 52 across 10 phases |
| **AGENTIC CORE modules** | 20 + 5 v2 modules |
| **Traits (core)** | 14 shared trait definitions |
| **AI providers** | 7 (Anthropic, OpenAI, Gemini, Grok, OpenRouter, Z.ai, MiniMax) |
| **Messaging channels** | 4 ([Telegram](docs/channels/telegram.md), [Discord](docs/channels/discord.md), [Slack](docs/channels/slack.md), [CLI](docs/channels/cli.md)) |
| **Agent tools** | 9 (shell, browser, file ops, web fetch, git, messaging, file transfer, memory manage, key manage) |
| **Encryption** | ChaCha20-Poly1305 + Ed25519 + AES-256-GCM (OTK) |
| **Memory backends** | 3 (SQLite, Markdown, failover) |
| **File storage** | 2 (local, S3/R2) |
| **Observability** | OpenTelemetry, 6 predefined metrics |
| **Security features** | Auto-whitelist, vault encryption, path traversal protection, force-push block, credential message deletion, 4-layer key validation, OTK secure key setup, secret output filter, UTF-8 safe string handling |
| **Vision support** | JPEG, PNG, GIF, WebP (Anthropic + OpenAI formats) — graceful fallback on text-only models |
| **Release profile** | `opt-level=z`, LTO, 1 codegen unit, stripped, `panic=unwind` |
| **Minimum Rust version** | 1.82 (Edition 2021) |
| **Binary size** | 7.1 MB (release, stripped, LTO) |
| **Memory (idle)** | 14 MB RSS |
| **Startup time** | < 1 second |

## Performance

SkyClaw is built for hyper-performance. Rust's zero-cost abstractions, async runtime, and aggressive release optimizations deliver server-grade capability at embedded-system resource usage.

| Metric | SkyClaw (Rust) | OpenClaw (TypeScript) |
|--------|---------------|----------------------|
| **Idle RAM** | 14 MB | ~1.2 GB |
| **Active RAM** | ~20 MB | 8 – 12 GB |
| **Binary / Install** | 7.1 MB single binary | 75 MB+ (npm + node_modules) |
| **Startup** | < 1 second | ~10 seconds (Node.js cold start) |
| **Long session (13h)** | Stable ~20 MB | 1.9 GB (memory accumulation) |
| **Runtime** | Native arm64/x86_64 | Node.js |
| **Dependencies** | 0 runtime deps (static binary) | npm ecosystem |

85x lower idle memory. Single binary deployment. No runtime dependencies.

## 3-Step Setup

### Step 1: Get a Telegram Bot Token

1. Open Telegram and search for [@BotFather](https://t.me/BotFather)
2. Send `/newbot`
3. Choose a name and a username (must end in `bot`)
4. BotFather replies with your bot token
5. Copy it

### Step 2: Deploy

```bash
git clone https://github.com/nagisanzenin/skyclaw.git
cd skyclaw
cargo build --release
export TELEGRAM_BOT_TOKEN="your-token-here"
./target/release/skyclaw start
```

### Step 3: Activate

1. Open your bot in Telegram
2. Send any message — SkyClaw sends you a secure setup link
3. Click the link, paste your API key in the browser form — it encrypts locally
4. Copy the encrypted blob back to chat — SkyClaw decrypts and validates
5. Or just paste a raw API key directly — SkyClaw auto-detects the provider

Supports: Anthropic, OpenAI, Gemini, Grok, OpenRouter, Z.ai, MiniMax

## Supported Providers

Paste any of these API keys in Telegram — SkyClaw detects the provider automatically:

| Key Pattern | Provider | Default Model |
|------------|----------|---------------|
| `sk-ant-*` | Anthropic | claude-sonnet-4-6 |
| `sk-*` | OpenAI | gpt-5.2 |
| `AIzaSy*` | Google Gemini | gemini-3-flash-preview |
| `xai-*` | xAI Grok | grok-4-1-fast-non-reasoning |
| `sk-or-*` | OpenRouter | anthropic/claude-sonnet-4-6 |
| *(explicit: `zai:KEY`)* | Z.ai (Zhipu) | glm-4.7-flash |
| *(config only)* | MiniMax | MiniMax-M2.5 |

## Channels

| Channel | Status | Feature Flag | Setup Guide |
|---------|--------|-------------|-------------|
| **Telegram** | Production | `telegram` | [docs/channels/telegram.md](docs/channels/telegram.md) |
| **Discord** | Production | `discord` | [docs/channels/discord.md](docs/channels/discord.md) |
| **Slack** | Production | `slack` | [docs/channels/slack.md](docs/channels/slack.md) |
| **CLI** | Built-in | — | [docs/channels/cli.md](docs/channels/cli.md) |

## Tools

| Tool | Description |
|------|-------------|
| **Shell** | Run any command on your server |
| **Browser** | Stealth headless Chrome — anti-detection patches, session persistence, navigate, click, type, screenshot |
| **File ops** | Read, write, list files on the server |
| **Web fetch** | HTTP GET with token-budgeted response extraction |
| **Git** | Clone, pull, push, commit, branch, diff, log |
| **Messaging** | Send real-time updates during multi-step tasks |
| **File transfer** | Send/receive files through messaging channels |
| **Memory manage** | Persistent knowledge CRUD — remember, recall, forget, update, list |
| **Key manage** | Generates secure setup links for API key onboarding — agent can send OTK links directly |

## Vision Support

SkyClaw can see and understand images. Send a photo through any channel — the runtime automatically:

1. Downloads the image to workspace
2. Base64-encodes it
3. Includes it as an image content part in the provider request
4. The LLM sees and analyzes the image

Supports Anthropic and OpenAI vision formats natively.

**Graceful fallback**: If images are sent to a text-only model (e.g. GPT-3.5, MiniMax, GLM text models), SkyClaw strips the images automatically, notifies the user, and continues processing the text. No API errors — just a helpful message suggesting a vision-capable model.

## Architecture

13-crate Cargo workspace:

```
skyclaw (binary)
├── skyclaw-core         Traits (13), types, config, errors
├── skyclaw-gateway      HTTP server, health, dashboard, OAuth identity
├── skyclaw-agent        AGENTIC CORE (20 modules)
├── skyclaw-providers    Anthropic, OpenAI-compatible
├── skyclaw-channels     Telegram, Discord, Slack, CLI
├── skyclaw-memory       SQLite + Markdown with failover
├── skyclaw-vault        ChaCha20-Poly1305 encrypted secrets
├── skyclaw-tools        Shell, browser, file ops, web fetch, git
├── skyclaw-skills       Skill registry (SkyHub v1)
├── skyclaw-automation   Heartbeat, cron scheduler
├── skyclaw-observable   OpenTelemetry, 6 predefined metrics
├── skyclaw-filestore    Local + S3/R2 file storage
└── skyclaw-test-utils   Test helpers
```

## Security

- **Auto-whitelist**: First user to message gets whitelisted. Everyone else denied.
- **Numeric ID only**: Allowlist matches on Telegram user IDs, not usernames.
- **Vault encryption**: ChaCha20-Poly1305 with vault:// URI scheme for secrets.
- **Path traversal protection**: File names sanitized, directory components stripped.
- **Force-push blocked**: Git tool blocks destructive operations by default.
- **Credential message deletion**: API keys and passwords are auto-deleted from chat history after reading.
- **OTK secure key setup**: API keys encrypted client-side via AES-256-GCM before transit. [Design doc](docs/OTK_SECURE_KEY_SETUP.md)
- **Secret output filter**: Hardcoded string-match censor prevents API keys from leaking in agent replies. System prompt enforces one-way secret flow (user → claw, never claw → user).

## Self-Configuration

Tell SkyClaw to change its own settings through natural language:

- "Change model to claude-opus-4-6"
- "Switch to GPT-5.2"

Or use the `/model` command for mechanical model switching:

- `/model` — show current model + all available models with `[vision]` tags
- `/model gpt-5.2` — switch instantly (validates model name, takes effect immediately)

The `/model` command is an escape hatch — it works even when the current model is too weak to follow self-configuration instructions. Proxy providers (OpenRouter, custom base_url) accept any model name.

Config lives at `~/.skyclaw/credentials.toml` — SkyClaw reads and edits this file itself.

## CLI Reference

```
skyclaw start              Start the gateway daemon
skyclaw chat               Interactive CLI chat
skyclaw status             Show running state
skyclaw update             Check for updates and rebuild
skyclaw config validate    Validate configuration
skyclaw config show        Print resolved config
skyclaw version            Show version info
```

### `skyclaw update`

Checks for new commits, pulls the latest code, and rebuilds the release binary in one command:

```bash
$ skyclaw update
SkyClaw Update
Current version: 2.0.1

Fetching latest changes...
3 new commit(s):

  a1b2c3d feat: LLM-based message classification
  d4e5f6g fix: orphaned tool_result in history pruning
  h7i8j9k docs: update CLI reference

Pulling from origin/main...
Building release binary... (this may take a few minutes)

Update complete!
Restart with: skyclaw start
```

Handles dirty working trees automatically (stash → pull → build → pop). If you're not in a git repo, it tells you.

## Development

```bash
cargo check --workspace                                    # Quick compilation check
cargo build --workspace                                    # Debug build
cargo test --workspace                                     # Run all 1217 tests
cargo clippy --workspace --all-targets --all-features -- -D warnings  # Lint (0 warnings)
cargo fmt --all                                            # Format
cargo build --release                                      # Release build
```

## Requirements

- Rust 1.82+
- Chrome/Chromium (for browser tool)
- A Telegram bot token

## Release Timeline

```
2026-03-11  v2.0.1  ●━━━ LLM chat/order classification — single LLM call classifies AND responds (chat = 1 call, order = instant ack + pipeline), abolished artificial tool iteration caps (budget + time are the guardrails), skyclaw update command, 1217 tests
                    │
2026-03-10  v2.0.0  ●━━━ AGENTIC CORE V2 — smart complexity classification (Trivial/Simple/Standard/Complex), prompt stratification (4 tiers), complexity-aware tool loop, execution profiles, structured failure types, 12% cheaper on compound tasks, 14% fewer tool calls, zero quality regression. Benchmarked: 20-turn A/B on GPT-5.2, 100% classification accuracy, 100% reliability. 1141 tests
                    │
2026-03-10  v1.7.0  ●━━━ Vision fallback & /model command — graceful image stripping for text-only models, /model mechanical switching with instant reload, model validation, hot-reload auto-revert, proxy provider flexibility, 1141 tests
                    │
2026-03-10  v1.6.0  ●━━━ Extreme resilience — zero panic paths, 26-finding hardening audit (22 fixed), send retry (3 attempts), dead worker respawn with message re-dispatch, SQLite 5s timeout, full catch_unwind coverage, lock poison recovery across all crates, graceful SIGTERM drain, 1130 tests
                    │
2026-03-10  v1.5.1  ●━━━ Crash resilience — 4-layer panic recovery, UTF-8 safety (6 fixes), conversation persistence, budget default fix, per-turn usage tracking
                    │
2026-03-09  v1.5.0  ●━━━ OTK secure key setup — AES-256-GCM encrypted onboarding, key_manage tool, secret output filter, SetupLinkGenerator trait, 1095 tests
                    │
2026-03-09  v1.4.0  ●━━━ Persistent memory & budget — memory_manage tool (CRUD), knowledge auto-injection, budget tracking, CLI chat, 1061 tests
                    │
2026-03-09  v1.3.0  ●━━━ Hyper-performance — 4-layer key validation, dynamic system prompt, placeholder defense, 1027 tests
                    │
2026-03-09  v1.2.0  ●━━━ Stealth browser — anti-detection patches, session persistence, credential deletion, 1012 tests
                    │
2026-03-08  v1.1.0  ●━━━ Provider expansion — 6 LLM providers, hot-reload, channel docs, path fixes
                    │
2026-03-08  v1.0.0  ●━━━ AGENTIC CORE — 35 features, 20 autonomy modules, vision support, 905 tests
                    │
2026-03-08  v0.9.0  ●━━━ Production hardening — Dockerfile, systemd, CI/CD, multi-user support
                    │
2026-03-08  v0.8.0  ●━━━ Telegram-native onboarding — API key validation, headless browser, self-config
                    │
2026-03-08  v0.7.0  ●━━━ Per-chat dispatcher — browser tool, stop commands, pending message injection
                    │
2026-03-08  v0.6.0  ●━━━ Agent autonomy — send_message tool, heartbeat system, configurable tool rounds
                    │
2026-03-08  v0.5.0  ●━━━ Agent tools — shell, file ops, file transfer, context management
                    │
2026-03-08  v0.4.0  ●━━━ SUSTAIN — docs, runbooks, skills registry, incident response
                    │
2026-03-08  v0.3.0  ●━━━ SHIP — security remediation (2C/6H/2M fixed), IaC, release workflow
                    │
2026-03-08  v0.2.0  ●━━━ HARDEN — 105 new tests, security audit, STRIDE threat model, deep code review
                    │
2026-03-08  v0.1.0  ●━━━ Wave A — gateway, providers, memory, vault, channels, full type system
                    │
2026-03-08  v0.0.1  ●━━━ Architecture scaffold — 13 crates, 12 traits, research documentation
```

## Community

Join the Discord to discuss, share feedback, and get help.

<a href="https://discord.gg/3ux2c5xz"><img src="https://img.shields.io/badge/Discord-Join%20Community-5865F2?style=for-the-badge&logo=discord&logoColor=white" alt="Join Discord"></a>

## Star History

[![Star History Chart](https://api.star-history.com/svg?repos=nagisanzenin/skyclaw&type=Date)](https://star-history.com/#nagisanzenin/skyclaw&Date)

## License

MIT
