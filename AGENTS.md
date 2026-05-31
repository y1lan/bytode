# bytode

Terminal coding agent (ReAct loop + DeepSeek + LSP + TUI) for Rust projects.

## Commands

- **Build:** `cargo build`
- **Test:** `cargo test` (single integration test in `tests/integration.rs`)
- **Smoke test:** `cargo run -- --help`
- **Run (interactive):** `cargo run` (REPL mode)
- **Run (one-shot):** `cargo run -- "your task"`
- **Run with custom model:** `cargo run -- --model deepseek-chat`

## Required Setup

- `DEEPSEEK_API_KEY` env var or `--api-key` flag.
- `ripgrep` on `$PATH` (for `search_code` tool).
- `rust-analyzer` on `$PATH` (for LSP diagnostics).
- Dependencies: standard Rust toolchain (Rust 2024 edition).

## Config

Three-tier cascade (TOML):
1. Code defaults
2. `~/.config/bytode.toml` (user)
3. `<project>/.bytode.toml` (project, highest priority)

Example config at project root overrides user config.

## Key Architecture

- **No shell tool.** Zero shell access. All operations go through typed tools.
- **Build output is JSON only.** Never parse text logs.
- **Diagnostics come from LSP only.** Never grep error output.
- **Memory compression is deterministic** (Rust code, not LLM-summarised) — triggers at >80% context usage.
- **Token estimation:** 1 tok ≈ 4 chars (no real tokenizer).
- **DeepSeek pricing:** $0.27/1M input, $1.10/1M output tokens.
- **Default model:** `deepseek-v4-pro`.
- **Writes are atomic:** tmp file + rename, with approval dialog by default.
- **`confirm_before_write` defaults to `true`.**

## Tool List

| Tool | Avail | Description |
|------|-------|-------------|
| `read_file` | Always | Read file with line numbers, supports offset/limit |
| `write_file` | Always | Atomic write with diff + approval |
| `search_code` | Always | `rg --json --line-number --no-heading` |
| `run_check` | Rust only | `cargo check --message-format json` with simple filter |
| `get_diagnostics` | Rust only | Reads LSP `publishDiagnostics` cache |

## Code Layout

```
src/
├── main.rs           # CLI (clap), wiring
├── config.rs         # TOML config, 3-tier loader
├── project.rs        # Marker-based language detection
├── error.rs          # BytodeError (thiserror)
├── agent/            # ReAct loop, context builder, memory
├── tools/            # Tool trait + 5 implementations
├── lsp/              # rust-analyzer client (JSON-RPC)
├── llm/              # DeepSeek client (async-openai)
└── ui/               # ratatui TUI
```

## Workflow

Normal cycle: `read_file → edit → run_check/get_diagnostics → repeat`. The SOP (Standard Operating Procedure) prompt injects after build failures, mandating diagnostics-first approach.
