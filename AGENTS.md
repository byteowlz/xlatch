# AGENTS.md

Guidance for coding agents working on this Rust workspace template.

## Domain and Architecture

- Read `CONTEXT.md` before domain work and use its canonical terms consistently.
- Update `CONTEXT.md` when domain language is resolved. Keep it a glossary only: no implementation details, specifications, or scratch notes.
- Read `docs/adr/` before architectural changes.
- Add an ADR only for a decision that is hard to reverse, surprising without context, and based on a real trade-off. Follow `docs/adr/README.md`.
- If the repository grows into multiple bounded contexts, replace the root glossary with a `CONTEXT-MAP.md` that links to context-local `CONTEXT.md` files.

## Workspace Map

```
Cargo.toml              # Workspace root: all deps pinned here, lint config
crates/
  xlatch-core/            # Shared library (the only lib crate)
    src/config.rs       #   AppConfig, LoggingConfig, RuntimeConfig, PathsConfig
    src/paths.rs        #   AppPaths, XDG resolution, write_default_config
    src/error.rs        #   CoreError, Result type alias
    src/schema.rs       #   JSON schema + example config generation & validation
    src/lib.rs          #   Public re-exports, APP_NAME const, env_prefix(), default_parallelism()
    examples/generate_config.rs  # Regenerates examples/ files from structs
  xlatch-cli/             # Unified xlatch executable: operator CLI, server, OS services
  xlatch-tui/             # TUI binary (ratatui, crossterm)
  xlatch-mcp/             # MCP server binary (rmcp 1.2, stdio transport)
examples/
  config.toml           # Generated example config (kept in sync by test)
  config.schema.json    # Generated JSON schema (kept in sync by test)
clippy.toml             # Clippy thresholds (complexity, doc-valid-idents)
justfile                # Task runner commands
release.toml            # cargo-release config (publish=false, push=false)
TUI.md                  # TUI architecture patterns reference
```

**Dependency flow**: All binaries depend on `xlatch-core`. No binary depends on another binary.

## Core Principles

- **Never publish** artifacts to public registries without explicit user approval.
- We favor clean refactors over backwards compatibility; update existing code in place (no `FooV2` suffixes).
- Target Windows 11, Linux, and macOS 14+ with the same behavior; no legacy OS shims.
- Keep file headers minimal — no author or timestamp banners.

## Rust Workflow

- Follow Clippy best practices: collapse trivial `if`s, inline `format!` arguments, and prefer method references over redundant closures.
- When tests compare structures, assert on the full value instead of individual fields.
- Run `cargo fmt` after code changes and `cargo test` for the touched crate. Invoke broader test or lint commands only if the user asks.

## Strict Lint Configuration

The workspace uses **maximum-strictness Clippy lints** (see `[workspace.lints.clippy]` in root `Cargo.toml`). Key constraints:

- `unsafe_code = "forbid"` — no unsafe anywhere
- `unwrap_used`, `expect_used`, `panic` = "deny" — use `?`, `anyhow::Result`, or `ok_or_else`
- `allow_attributes` = "deny" — cannot add `#[allow(...)]` to suppress warnings
- `dbg_macro`, `todo`, `unimplemented` = "deny" — no placeholder code
- `exit` = "deny" — return errors from `main()` instead
- `print_stdout/print_stderr` = "allow" — CLIs/TUIs/APIs need output

**When adding new code**: use `anyhow::Result<()>` for fallible functions, propagate errors with `?`, use `.context("message")?` for better error messages. Never `unwrap()`.

## Config Schema Workflow

When you modify `AppConfig` or any config struct in `xlatch-core/src/config.rs`:

1. Run `just generate-config` to regenerate `examples/config.toml` and `examples/config.schema.json`
2. The test `validate_examples_are_up_to_date` will fail if you forget this step
3. Run `just test` to verify

## Common Agent Tasks

### Adding a new CLI subcommand

1. Add a variant to the `Command` enum in `crates/xlatch-cli/src/main.rs`
2. Add a corresponding `#[derive(Debug, Args)]` struct for its arguments
3. Add a `handle_*` function and wire it in `try_main()`
4. Support `--json` output in the handler

### Adding a new MCP tool

1. Add a method to the `#[tool_router] impl McpServer` block in `crates/xlatch-mcp/src/main.rs`
2. Use `#[tool(description = "...")]` attribute
3. Define a params struct with `#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]`
4. Accept params as `Parameters(params): Parameters<YourParams>`
5. Return `Result<CallToolResult, McpError>`
6. The tool is auto-registered via the `tool_router` macro — no manual registration needed

### Adding a new API endpoint

1. Add a handler function in `crates/xlatch-cli/src/server.rs`
2. Add the route in the `Router::new()` chain
3. Use `State(state): State<AppState>` to access config

### Adding a config field

1. Add the field to the appropriate struct in `crates/xlatch-core/src/config.rs`
2. Add `#[schemars(...)]` annotations for schema metadata
3. Set a default in the `Default` impl
4. Run `just generate-config` to update example files
5. Run `just test` to verify

## CLI Expectations

- Prefer subcommands for verbs and keep outputs quiet/verbose via standard flags (`-q`, chainable `-v`, `--debug`, `--trace`).
- Support machine-readable modes via `--json` and honor NO_COLOR/FORCE_COLOR.
- Offer `--dry-run`, `--yes/--force`, `--no-progress`, `--timeout`, and `--parallel` when operations warrant them.
- Generate help quickly (`-h/--help`) and provide shell completions off the same Clap definitions.

## MCP Crate (rmcp 1.2)

The MCP server uses the official `rmcp` crate (Rust SDK for Model Context Protocol).

**Key patterns:**

- `#[tool_router]` on impl block auto-generates tool routing
- `#[tool_handler]` on `ServerHandler` impl wires tools into the server
- `ServerInfo` is non-exhaustive — use `let mut info = ServerInfo::default(); info.field = value;`
- Transport: `rmcp::transport::io::stdio()` for stdio-based MCP
- Re-exports: `rmcp::schemars::JsonSchema` and `rmcp::serde::{Serialize, Deserialize}` are available

## Configuration & Storage

- Use XDG directories when available: config at `$XDG_CONFIG_HOME/<app>/config.toml`, data at `$XDG_DATA_HOME/<app>`, state at `$XDG_STATE_HOME/<app>` with sensible fallbacks (e.g., `~/.config`).
- Expand `~` and environment variables in config paths.
- Ship a commented example under `examples/`, create a default config on first run, and load overrides via the `config` crate.
- Environment variable override prefix: `RUST_WORKSPACE__` (double underscore for nesting, e.g., `RUST_WORKSPACE__LOGGING__LEVEL=debug`).

## Rules

- Do exactly what the user asks — no unsolicited files or docs.
- Keep README updates concise, emoji-free, and only when requested.
- Never commit secrets or sensitive paths; scrub logs before surfacing them.

## Justfile Commands

This project uses [just](https://github.com/casey/just) as a command runner. Run `just` to see available commands.

**Core commands:**

```bash
just              # Show available commands
just install-all  # Install all binaries
just install-crate CRATE # Install specific crate
just build        # Debug build (all crates)
just build-release # Release build (all crates)
just test         # Run all tests
just fmt          # Format all code
just clippy       # Run linter on all crates
just check-all    # Format + lint + test
```

**Workspace navigation:**

```bash
just list         # List all crates
just list-bins    # List binary crates
just list-libs    # List library crates
just build-crate CRATE  # Build specific crate
just test-crate CRATE   # Test specific crate
just clippy-crate CRATE # Lint specific crate
```

**Development workflow:**

```bash
just check        # Fast compile check
just fix          # Auto-fix clippy warnings
just docs         # Generate documentation
just update       # Update dependencies
```

Always run `just check-all` before committing significant changes.

## Issue Tracking (trx)

Use `trx` for all issue tracking. Do NOT use markdown TODOs or external trackers.

```bash
trx ready --json                              # Find unblocked work
trx create "Title" -t task -p 2 --json        # Create issue (types: bug/feature/task/epic/chore)
trx update <id> --status in_progress --json   # Claim task
trx close <id> --reason "Done" --json         # Complete work
```

Priorities: 0=critical, 1=high, 2=medium (default), 3=low, 4=backlog

## Releases & Distribution

This project uses GitHub Actions for automated releases. See `.github/workflows/release.yml`.

**Creating a release:**

```bash
# Tag-based (automatic trigger)
git tag v1.0.0 && git push --tags

# Manual trigger via CLI
gh workflow run release.yml -f tag=v1.0.0
```

**What the workflow builds:**

- Linux x86_64 (ubuntu-latest)
- macOS x86_64 (cross-compiled from macos-14 ARM64)
- macOS ARM64 (macos-14)
- Windows x86_64 (if enabled)

**Disabled by default (uncomment in workflow if needed):**

- Linux ARM64: Requires `Cross.toml` with OpenSSL configuration
- Windows: May have C runtime mismatch issues with some crates

**Platform notes:**

- `macos-13` runner is retired - always use `macos-14`
- For protobuf projects: uncomment the protoc installation steps
- For ML projects: uncomment `--features coreml` for Apple Silicon
- Workspace builds package all binaries matching `xlatch*`

**Required secrets for package managers:**

- `TAP_GITHUB_TOKEN` - PAT with repo access to byteowlz/homebrew-tap
- `AUR_SSH_PRIVATE_KEY` - SSH key registered with AUR
- `AUR_EMAIL` - Email for AUR commits

Use `byt secrets setup <repo>` to configure secrets.

**Installation methods (once published):**

```bash
# Homebrew (macOS/Linux)
brew install byteowlz/tap/<binary-name>

# AUR (Arch Linux)
yay -S <binary-name>

# Scoop (Windows)
scoop bucket add byteowlz https://github.com/byteowlz/scoop-bucket
scoop install <binary-name>
```

---

## Supply Chain Security

- **Lock files**: `Cargo.lock` is committed and pins exact versions. Always commit lock file changes.
- **Audit regularly**: Run `cargo audit` to check for known vulnerabilities in dependencies.
- **Vet crates**: Consider `cargo-vet` to track which crates have been reviewed.
- **Minimal dependencies**: Favour std library before adding third-party crates. Rust crates have no install scripts (lower risk than npm/PyPI), but transitive deps still matter.

## Data Formats

Use JSON or TOML only for application data, configuration, and CLI output. Do not add YAML support.

## Service and Commit Conventions

Expose daemon lifecycle through `xlatch service run|start|stop|restart|enable|disable|status`. Use one `xlatch` executable for operator commands and the HTTPS daemon. Commit small, coherent changes after relevant checks pass.
