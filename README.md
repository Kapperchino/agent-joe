# Agent Joe

An open source TUI-based coding tool that only works with rust and does not have access to the shell.

## Why

I just hate the fact that cli tools with connections to llm providers can execute any arbitrary command on my terminal.

With the rust only requirement, I can also reduce the number of possible actions to just rust specific ones, lowering
the total tool count.

And this is a fun project to work on.

## State

Works pretty well currently, still doesn't do as good of a job as codex, I think the main reason being the prompts not
being as good and not having a plan mode.

The [implementation plan](IMPLEMENTATION_PLAN.md) covers the runtime, context, validation, and collaboration
improvements, with dependencies and acceptance criteria.

## Flags

The tui supports some flags

| Flag     | Description                                                     |
|----------|-----------------------------------------------------------------|
| --debug  | Adds significantly more logging and prints out thinking tokens. |
| --simple | Multi-agent mode will be disabled, will work just like codex    |

## Sessions

Conversations are saved automatically in a project-local LMDB environment at
`.turbo-code/sessions`. The same session runtime serves simple and worker modes.

- `/sessions` lists saved conversations and their IDs.
- `/resume` opens a searchable picker of saved conversations in this project,
  most recently updated first. Use arrow keys to select, Enter to resume, and Esc
  to cancel. Selecting a session restores its transcript without starting a turn.
- `/resume <id>` restores a conversation while idle. Send a message to continue.
- `/new` starts a fresh conversation while idle.
- `/clear` cancels active work and starts a fresh conversation. Previous sessions
  remain available; it does not delete saved history or undo workspace changes.

Resume uses current instructions and project access policy. A saved session must
match the current workspace identity and provider route. Saved tool calls never
execute automatically; calls with intent but no completion are marked uncertain
and require inspecting the workspace before retrying. Worker sessions stay linked
to their parent conversation.

Storage uses private permissions and is inaccessible to model-facing file tools.
Events and snapshots commit together; a crash discards uncommitted transactions.
LMDB also stores exclusive session ownership. Claims and releases are atomic;
each owner records its process identity and a unique token. Resume can reclaim
ownership after that process exits, and stale handles cannot read or update the
session or release a newer owner's claim.
Provider credentials and authorization headers are not serialized. The LMDB map
currently allows 1 GiB; storage errors stop automatic continuation. Context
budgeting, output artifacts, `/fork`, and `/compact` are still planned.

## Supported llm providers

| Provider   | Support                                                |
|------------|--------------------------------------------------------|
| OpenAI     | Fully supported                                        |
| Codex      | Fully supported                                        |
| Claude     | Should work, haven't tested it in a minute due to cost |
| OpenRouter | Does not support web_search but everything should work |
| Local      | Response api fully supported                           |

## Build & Run

```sh
cargo run --relase
```

## Keybindings

The TUI is similar to claude code and codex with one major difference. Vim bindings are foced upon you.

## Tools

TBD
