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
- `/fork` creates and switches to an independent copy of the current conversation
  while idle. Both conversations use the same workspace; edits are shared.
- `/compact` summarizes older context while idle without deleting the transcript.
  Compaction also runs automatically between complete exchanges as context fills.
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
allows 1 GiB; storage errors stop automatic continuation. Existing session snapshots
remain readable. Oversized inline tool outputs from older sessions become artifacts
when resumed.

Tool outputs larger than 8 KiB are saved as immutable artifacts, with a short
preview and artifact ID in the conversation. The agent can use `read_artifact`
to retrieve UTF-8 pages of up to 4096 bytes, following `next_offset`. Parents can
retrieve their workers' artifacts; forks retain the artifacts available when they
were created. File reads are limited to 16 MiB, search output to 32 MiB, and process
output to 16 MiB per stream. Full output within these limits is retained; each
artifact has a 64 MiB maximum. Search context is limited to 1000 lines on each side.

Request context is separate from the saved transcript. Current instructions,
verbatim user messages, the two most recent complete exchanges, pending questions,
and validation/failure evidence take priority over optional workspace context.
Older exchanges use provider-native compaction on the public OpenAI route, or a
model-generated summary on other routes. Native windows, including retained and
opaque items, are replayed intact. Compaction cannot split tool calls from results
or rerun saved operations. Failed, incomplete, or oversized summaries leave the
previous context intact; interrupt and clear cancel an in-flight compaction.

The status line separates estimated context for the next request from cumulative
provider token usage, including compaction. Estimates conservatively count serialized
UTF-8 bytes plus framing rather than using a model-specific tokenizer. Automatic
compaction starts at 80% of the input budget after reserving response space. If
mandatory context cannot fit, the turn stops with recovery instructions instead
of dropping requirements.

Runtime settings are separate from provider credentials:

| Flag | Default | Purpose |
| --- | --- | --- |
| `--context-tokens` | `128000` | Context ceiling; configure it for the selected model |
| `--response-tokens` | `16000` | Reserved response space; sent as an output limit where the route accepts it |
| `--native-compaction` | `auto` | `auto` uses native compaction on public OpenAI; `on` opts a compatible route in; `off` uses summaries |
| `--session-namespace` | `sessions` | Protected storage namespace under `.turbo-code` |

The Codex backend does not receive `max_output_tokens`; response space is still
reserved locally. Native compaction failures stop continuation and can be retried
with `/compact`. Opaque native state cannot be converted to a fallback text summary.
If preserved requirements or recent exchanges exceed the budget, restart with a
larger context setting supported by your model, or use `/new`. Pending question
records survive resume, fork, and compaction; interactive question tools are part
of the planned M9 work.

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

## Tests

Run the full suite from a regular terminal or a CI host that permits creating
process sandboxes:

```sh
cargo test --workspace --offline
```

The Cargo-cancellation and process-isolation tests launch their own sandboxed
processes. On macOS, running them through agent-joe's Cargo tool or another
restricted sandbox runner can fail with
`sandbox-exec: sandbox_apply: Operation not permitted` because the parent sandbox
prevents applying the test's sandbox. Run these tests outside that runner. The
Cargo-cancellation test can be run directly with:

```sh
cargo test -p actors runtime_test::interrupt_reaps_a_running_cargo_process_before_publishing_cancelled -- --exact
```

Linux process-isolation tests require `/usr/bin/bwrap` and host namespace support.

## Keybindings

The TUI is similar to claude code and codex with one major difference. Vim bindings are foced upon you.

## Tools

TBD
