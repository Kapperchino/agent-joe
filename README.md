# Joe Code

```text
    _~^~^~_
\) /  o o  \ (/
  '_   -   _'
  / '-----' \
```

**Small claws. Big ideas.** Ferris is Joe Code's mascot and your Rust coding companion.
The terminal UI pairs crab-orange accents with warm charcoal surfaces and cream text.
Success and added lines stay green; warnings are amber, and errors and removed lines stay red.

An open source TUI-based coding tool that only works with rust and does not have access to the shell.

## Why

I just hate the fact that cli tools with connections to llm providers can execute any arbitrary command on my terminal.

With the rust only requirement, I can also reduce the number of possible actions to just rust specific ones, lowering
the total tool count.

And this is a fun project to work on.

## State

The runtime milestones through M9 include sessions, bounded workers, guarded
change review, plan mode, tracked steps, and structured user questions. Skills
and controlled MCP integrations remain planned.

## Actor and turn runtime

Each worker actor owns a turn state machine. The actor mailbox serializes input,
provider events, tool results, and cleanup notifications; the turn machine decides
which transitions and effects are accepted. The turn driver executes those effects
without moving turn ownership into background tasks.

An explicit actor stop follows the same lifecycle: cancel queued work, clean up the
active turn while still receiving final tool results, drain the session, then stop
the actor. New turns are not accepted or persisted once closing begins. Direct
actor stops retain a post-stop cleanup fallback when mailbox delivery is unavailable.

## Supported llm providers

| Provider   | Support                                                |
|------------|--------------------------------------------------------|
| OpenAI     | Fully supported                                        |
| Codex      | Fully supported                                        |
| Claude     | Should work, haven't tested it in a minute due to cost |
| OpenRouter | Does not support web_search but everything should work |
| Local      | Response api fully supported                           |

Context compaction runs automatically as the conversation approaches its token
budget, or manually with `/compact` while idle. The default
`--native-compaction auto` uses streaming native compaction with Codex login,
including Astra, and the standalone compaction endpoint with the public OpenAI
API. Other providers use conversation summaries. `--native-compaction off`
selects summaries for sessions that do not already contain encrypted context.
The saved transcript remains available after compaction.

## Build & Run

```sh
cargo build --release
cargo run --release -p turbo-code
```

The default build compiles Joe and the `joe-sandbox` binary from the `sandbox`
package. Keep both executables in the same directory when installing Joe. Other
applications using the sandbox crate can set `JOE_SANDBOX_LAUNCHER` to the launcher
path. Workspace checks and tests include the launcher.

The first sandbox operation prepares the Linux guest image, guest init program,
and native components, then caches a copy of the built launcher outside the
workspace. On macOS, this copy is signed with hypervisor entitlements. Setup needs
network access and a C compiler, plus Rust's linker tools on macOS and `make` on
Linux. It does not invoke Cargo or compile the Rust launcher.

Cargo operations automatically download missing crates.io dependencies on the
host, verify their checksums, and cache them for the sandbox. This also works
without a lockfile and after dependencies change. Builds, build scripts, tests,
and programs run inside the sandbox without network access. Git dependencies and
custom registries are not downloaded automatically.

## Tests

Each crate keeps its tests and fixtures in its own `tests/` directory. Unit tests
live under `tests/unit/`, organized by module. For example,
`src/actors/src/batch.rs` has its tests in `src/actors/tests/unit/batch/tests.rs`.
They are connected with `#[cfg(test)]` and `#[path]` so they retain access to
private module items. Integration tests live directly under the crate's `tests/`
directory, such as `src/sandbox/tests/launcher.rs`, where Cargo discovers them
automatically.

```sh
cargo test --workspace
```

## Keybindings

The TUI is similar to claude code and codex with one major difference. Vim bindings are foced upon you.
