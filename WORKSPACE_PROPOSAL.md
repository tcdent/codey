# Workspace Crate Structure Proposal

## Problem

The single-crate setup uses **88 `cli` feature gates** and **14 `profiling` gates**
across 11 files to support both library and CLI use cases. The heaviest files:

| File | `cli` gates | What's gated |
|---|---|---|
| config.rs | 26 | Full Config struct + every sub-config type |
| transcript.rs | 20 | All render() methods + ratatui imports |
| effect.rs | 18 | EffectQueue, PendingEffect, CLI effect variants |
| tools/mod.rs | 8 | Tool impls, handlers, browser, registry constructors |
| tools/pipeline.rs | 3 | handlers usage, Block import |
| ide/mod.rs | 2 | nvim module/re-export |
| lib.rs | 4 | compaction, notifications, prompts, tool_filter modules |

Additionally, `main.rs` re-declares every module independently of `lib.rs`,
maintaining two parallel module trees.

## Proposed Structure: 5 Crates + Library Facade

```
codey/
├── Cargo.toml                  # [workspace] + "codey" library facade
├── crates/
│   ├── codey-core/             # Foundation: agent, transcript, types
│   ├── codey-tools/            # Built-in tool impls + effect handlers
│   ├── codey-ide/              # IDE backend impls (nvim)
│   ├── codey-tui/              # Terminal UI rendering
│   └── codey-cli/              # Binary: app loop, config, commands
```

### Dependency Graph

```
codey-core          (no internal deps)
    ↑
codey-ide           (core)
    ↑
codey-tools         (core)
    ↑
codey-tui           (core, tools)
    ↑
codey-cli           (core, tools, ide, tui)

codey [lib facade]  (core)
```

---

## Crate Details

### `codey-core` — The engine (0 feature flags)

Everything currently shared between lib and cli, minus rendering.

**Contents:**
- `AgentRuntimeConfig`, constants (`CODEY_DIR`, etc.) — from config.rs
- OAuth credential management — from auth.rs
- `Agent`, `AgentStep`, `RequestMode`, `Usage`, LLM client, agent registry — from llm/
- `Block` trait (data-only, no render), `TextBlock`, `ThinkingBlock`, `ToolBlock`,
  `NotificationBlock`, `Turn`, `Transcript`, `Stage`, persistence, macros — from transcript.rs
- `Ide` trait + core types (`Edit`, `ToolPreview`, `Selection`, `IdeEvent`) — from ide/mod.rs
- Core `Effect` variants (`AwaitApproval`, `Ide*`, background task effects),
  `EffectResult` — from effect.rs
- `Tool` trait, `ToolPipeline`, `Step`, `EffectHandler` — from tools/pipeline.rs
- `SimpleTool`, `ToolRegistry` (with `empty()` + `register()`), `ToolCall`,
  `ToolExecutor` — from tools/mod.rs
- File I/O utilities — from tools/io.rs

**Deps:** genai, tokio, serde, serde_json, chrono, typetag, anyhow, thiserror,
reqwest, async-trait, tracing, sha2, base64, uuid, dirs, url, urlencoding, rand,
dotenvy, toml

This is what library users depend on. No ratatui, no crossterm, no nvim-rs,
no chromiumoxide.

---

### `codey-tools` — Built-in tools (0 feature flags)

All concrete tool implementations, currently behind `#[cfg(feature = "cli")]`.

**Contents:**
- All tool impls: `ReadFileTool`, `WriteFileTool`, `EditFileTool`, `ShellTool`,
  `FetchUrlTool`, `FetchHtmlTool`, `WebSearchTool`, `OpenFileTool`,
  `SpawnAgentTool`, `ListAgentsTool`, `GetAgentTool`, `RecordCorrectionTool`
  — from tools/impls/
- All effect handlers (`ValidateFile`, `AwaitApproval`, `Output`, `WriteFile`,
  `ShellExec`, etc.) — from tools/handlers.rs
- Browser context initialization — from tools/browser/
- `ToolFilterConfig` + compiled filter logic — from tool_filter.rs
- Pre-populated registries: `ToolRegistry::new()`, `::subagent()`,
  `::read_only()`
- Extended `Effect` variants: `SpawnAgent`, `ListAgents`, `GetAgent`
- Tool-specific block types (defined via macros)

**Deps:** codey-core, chromiumoxide, readability, htmd, fancy-regex, open, tokio

---

### `codey-ide` — IDE backends (0 feature flags)

Isolates nvim-rs (and future editor backends) from everything else.

**Contents:**
- Neovim RPC integration + Lua helper scripts — from ide/nvim/

**Deps:** codey-core (for `Ide` trait), nvim-rs, tokio, anyhow

---

### `codey-tui` — Terminal UI rendering (0 feature flags)

All ratatui/crossterm rendering, extracted from current
`#[cfg(feature = "cli")] fn render()` methods.

**Contents:**
- `ChatView`, `InputBox`, snapshot tests — from ui/
- All render() impls for `TextBlock`, `ThinkingBlock`, `ToolBlock`,
  `NotificationBlock`, plus helpers (`render_prefix`, `render_agent_label`,
  `render_approval_prompt`, `render_result`) — from transcript.rs
- `CompactionBlock` — from compaction.rs

**Deps:** codey-core, codey-tools (for tool-specific block types), ratatui,
crossterm, ratskin, textwrap, unicode-width

---

### `codey-cli` — The binary (0 `cli` flags, optional `profiling`)

Everything that wires the app together.

**Contents:**
- Entry point, CLI arg parsing — from main.rs
- `App` struct + main event loop — from app.rs
- Full `Config` struct (`GeneralConfig`, `AgentsConfig`, `UiConfig`,
  `ToolsConfig`, `IdeConfig`, `BrowserConfig`, `AuthConfig`,
  `AgentPersonaConfig`) + `Config::load()` — from config.rs
- `EffectQueue`, `PendingEffect`, `Resource`, `EffectPoll` — from effect.rs
- Slash commands — from commands.rs
- System prompt generation — from prompts.rs
- Notification queue — from notifications.rs
- Performance profiling (behind optional `profiling` feature) — from profiler.rs

**Deps:** all crates + clap, sysinfo (optional)

---

### `codey` — Library facade (root crate)

Thin re-export preserving the existing public API:

```rust
pub use codey_core::{Agent, AgentStep, AgentRuntimeConfig, RequestMode, Usage};
pub use codey_core::{SimpleTool, ToolCall, ToolRegistry};
```

---

## Refactoring Required

Three design knots to untangle during the split:

### 1. `Block::render()` decoupling

Currently `render()` lives on the `Block` trait gated by `#[cfg(feature = "cli")]`.
In the workspace, the trait in codey-core would be data-only. Rendering moves to
codey-tui as either a separate `RenderBlock` trait or a
`render_block(&dyn Block, width) -> Vec<Line>` dispatcher.

The `define_tool_block!` / `define_simple_tool_block!` macros would split into
data generation (codey-tools) and render generation (codey-tui).

### 2. `Tool::create_block()` decoupling

Same pattern — this method returns `Box<dyn Block>` for the TUI to render. It
could move to a parallel trait in codey-tui, or tools could return a struct that
codey-tui knows how to turn into a block.

### 3. `Effect` enum split

`SpawnAgent { agent: Agent }` creates a dependency from the effect system back to
the Agent type. Options:
- Keep core variants in codey-core; add `SpawnAgent`/`ListAgents`/`GetAgent` as
  a `CliEffect` enum in codey-tools (since those only apply in multi-agent CLI)
- Use type erasure (`Box<dyn Any>`) for the agent field in a unified enum

---

## Testing Benefits

### Current state

141 tests, all inline `#[cfg(test)]` modules — no integration test directory.
19 tests are behind `#[cfg(all(test, feature = "cli"))]` and won't run in
library-only builds. Tests that touch the filesystem, network, shell, and
neovim RPC all live in the same compilation unit.

| Category | Tests | External deps |
|---|---|---|
| Pure unit (queues, parsing, filters) | 63 | None |
| Filesystem (tempfile) | 19 | Temp files |
| Shell execution | 22 | `echo`, `ls` |
| Network (HTTP/browser) | 4 | Real URLs + chromiumoxide |
| Neovim RPC | 3 | Running nvim (`#[ignore]`) |
| UI snapshots (insta) | 8 | None (TestBackend) |
| UI widget logic | 22 | None |

### What the workspace unlocks

**1. Feature-gated tests disappear**

The 19 tests behind `#[cfg(all(test, feature = "cli"))]` become normal tests in
their respective crates. No more worrying about whether `cargo test` ran
everything or only the non-gated subset.

**2. Per-crate `cargo test -p` for fast iteration**

`cargo test -p codey-core` skips compiling ratatui, chromiumoxide, crossterm,
and nvim-rs entirely. On a project where the full build pulls in ~180 crates,
this is a meaningful compile-time win during development.

**3. Integration tests at crate boundaries**

Each crate gets its own `tests/` directory for testing the public API contract.
This catches breaking interface changes early — especially important since
codey-core is the foundation everything else builds on.

Proposed per-crate integration tests:

```
crates/codey-core/tests/
├── agent_lifecycle.rs      # Create agent, send request, iterate steps
│                           #   (mock LLM or record/replay)
├── transcript_persistence.rs  # Save → load → roundtrip with real JSON files
├── tool_executor.rs        # Build pipelines with mock tools, verify step
│                           #   sequencing, foreground/background ordering
└── tool_registry.rs        # Register, get, iterate, verify schema output

crates/codey-tools/tests/
├── file_tools.rs           # ReadFile/WriteFile/EditFile end-to-end with
│                           #   tempfiles — compose pipeline → execute →
│                           #   verify filesystem state
├── shell_tool.rs           # ShellTool pipeline with real commands
├── tool_filters.rs         # ToolFilterConfig against real tool params
└── registries.rs           # Verify new()/subagent()/read_only() contain
│                           #   expected tools, no duplicates, valid schemas

crates/codey-ide/tests/
└── nvim_integration.rs     # Connect → preview → diff → close lifecycle
                            #   (#[ignore] without CODEY_TEST_NVIM=1)

crates/codey-tui/tests/
├── chat_rendering.rs       # Render full conversation turns with real
│                           #   transcript data, snapshot the output
├── block_rendering.rs      # Each block type renders correctly at various
│                           #   widths (narrow terminal, wide terminal)
└── input_sequences.rs      # Multi-step input interactions: type, navigate,
                            #   paste, undo — verify final buffer state

crates/codey-cli/tests/
├── config_loading.rs       # Load from tempfile TOML, verify all sections
│                           #   parse, defaults fill in, CLI overrides apply
├── effect_queue.rs         # EffectQueue approval/resource exclusivity
│                           #   (currently unit tests, better as integration
│                           #   tests against PendingEffect public API)
└── commands.rs             # Slash command parsing and dispatch
```

**4. External dependency isolation in CI**

Tests that hit the network or need external services are confined to specific
crates. CI can run the matrix:

```yaml
# Fast lane — runs on every push (~15s)
- cargo test -p codey-core
- cargo test -p codey-tui

# Standard lane — runs on every push (~30s)
- cargo test -p codey-tools    # filesystem + shell
- cargo test -p codey-cli

# Integration lane — runs on PR only
- cargo test -p codey-tools -- fetch   # network tests
- cargo test -p codey-ide -- --ignored # needs nvim
```

The fast lane never touches the filesystem, network, or shell. A flaky
`fetch_url` test can't block an unrelated transcript bugfix.

**5. Snapshot tests stay focused**

The 8 insta snapshots + 44 input widget tests move to codey-tui. They only
recompile when TUI code changes. `cargo insta review` scopes naturally to the
crate you're working in.

**6. Doc tests on the library facade**

The `codey` facade crate can have doc tests that verify the public API works
end-to-end from a library consumer's perspective — the same example from the
current lib.rs doc comment, but now it compiles against the real published
interface rather than testing internals.

---

## What This Eliminates

- **All 88 `cli` feature gates** — replaced by crate boundaries
- **Dual module declarations** in lib.rs vs main.rs — each crate owns its modules
- **14 `profiling` gates** collapse to a single optional feature on codey-cli
- **Heavy deps isolated** — library users never pull in chromiumoxide, ratatui,
  crossterm, or nvim-rs
- **19 feature-gated tests** — become normal tests in their own crates
- **Flaky test coupling** — network/browser tests can't block core logic tests
