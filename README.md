# Agent Joe

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
cargo run --relase
```

Building Joe does not download or compile the sandbox launcher. The first sandbox
operation prepares it on the host and caches it for later runs. This setup needs
network access, Cargo, a Rust compiler, a C compiler, and `make` on Linux. Cargo
commands inside the sandbox remain offline.

## Keybindings

The TUI is similar to claude code and codex with one major difference. Vim bindings are foced upon you.
