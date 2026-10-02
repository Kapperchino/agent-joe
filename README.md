# Joe Code

An open-source terminal coding assistant for Rust. Joe reads and edits your project,
manages Git changes, and runs Cargo through typed tools—not an arbitrary shell.
Builds, tests, and programs run in a network-isolated sandbox.

## Quick start

Requires Rust, Git, and Apple Silicon macOS or Linux with KVM access. Initial
sandbox setup needs network access and a C compiler, plus Rust's linker tools on
macOS or `make` on Linux.

Build from this repository:

```sh
cargo build --release
```

Keep `target/release/turbo-code` and `target/release/joe-sandbox` together. Launch
Joe from the Rust project you want to work on:

```sh
cd /path/to/your/project
/path/to/agent-joe/target/release/turbo-code
```

On first launch, follow the setup screen to choose Claude, OpenAI (API key or
Codex login), OpenRouter, or a local OpenAI-compatible server.

Read-only sessions use the existing checkout without creating a worktree. The
first write operation creates a separate session worktree based on a committed
local `main` branch, leaving existing checkout edits untouched. Delegated writers
share that session worktree; merging back requires your approval. Forks and plan
handoffs read their source worktree until their own first write.

## Usage

- Describe a task and press `Enter`. Editing uses Vim-style bindings; `Esc`
  returns to normal mode, where `q` quits.
- `/plan` investigates without changing files; `/implement` enables changes.
- `/diff` reviews changes; `/sessions` and `/resume` reopen saved conversations.
- `/questions` opens pending questions; `/compact` condenses older context.
- `/agent` or `Ctrl+g` opens the agent-thread picker. Each sub-agent has its own
  live output, tool calls, objective, status, and unread indicator; none of that
  output is mixed into the main conversation. Select a thread with `↑`/`↓` and
  `Enter`, scroll with `PgUp`/`PgDn` or `Home`/`End`, and press `Esc` to return to
  Main. Agent views are read-only and preserve your unsent main-conversation draft.
- `Ctrl+o` toggles tool history. Run `turbo-code --help` for CLI options.

## Security

- **Restricted tools:** The agent has no general-purpose shell tool. File operations
  stay within the project boundary and enforce protected paths.
- **Isolated execution:** Cargo builds, build scripts, tests, and programs run in a
  Linux VM without network access. Execution is refused if isolation is unavailable;
  missing crates.io dependencies are downloaded separately on the host.
- **Provider access:** Prompts and tool results, including source excerpts, are sent
  to your configured model provider. Sandbox isolation does not make them private.

Keep secrets out of prompts and accessible project files; `.gitignore` is not an
access-control boundary. Review generated changes before merging.
