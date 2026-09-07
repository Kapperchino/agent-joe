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

## Repository discovery and instructions

Discovery uses a sorted file inventory independent of Rust symbols. It includes
manifests, Markdown, CI files, fixtures, empty files, and hidden files. Nested
`.gitignore` and `.ignore` files apply from the project root downward: deeper
rules take precedence, `.ignore` takes precedence within a directory, and the
last matching rule wins. Negation cannot reopen an excluded parent directory.
Discovery always excludes `.git`, `target`, and protected Joe storage. It does
not consult ignore files outside the project or user Git configuration. Explicit
file reads and directory listings can inspect ignored paths when policy allows.
Symlinks, hard links, special files, and protected storage remain inaccessible.

- `find_files` searches relative filenames/paths, using literal matching by
  default or regex with `literal: false`.
- `grep` searches discoverable UTF-8 text, using regex by default or
  `literal: true`. It returns matching one-based line numbers and optional
  surrounding lines. Binary and unreadable files are counted as skipped; up to
  100 skipped files include reasons.
- Both searches accept `include` and `exclude` as newline-separated path globs.
  `*` matches within a path component; `**` crosses directories. Excludes win.
  Results default to 200 and accept a `limit` from 1 to 1000. `truncated` and
  `truncation` identify omitted results and whether the result or output limit
  was reached. Narrow the pattern or filters to retrieve other matches.
- `list_directory` returns sorted pages with `total`, `truncated`, and
  `next_offset`. Pass that offset for the next page. It shares the 200 default
  and 1000 maximum. `read_file` on a directory returns the first page.
- `read_file` reads directly from disk. Ranges use a one-based inclusive `start`
  and exclusive `end`: `{ "start": 2, "end": 4 }` reads lines 2 and 3.
  Ends beyond EOF are clamped; invalid or nonexistent starts return structured
  errors with the requested range and available line count when known.
  Reading an empty file without a range succeeds. Non-Rust and newly created
  files need no semantic index. Rust symbol ranges use the same convention.

Search output is bounded to 32 MiB including space reserved for metadata. An
inventory scan stops explicitly above 250,000 visited entries or 32 MiB of file
paths; add ignore rules to reduce that inventory. Both modes share watcher
startup and cleanup. Create, modify, delete, and rename events refresh analysis;
successful edits refresh it immediately. Discovery and reads consult disk, and
semantic context refreshes before use, so watcher lag cannot hide new content.

Global guidance comes from `~/.turbo-code/AGENTS.md`; `--instructions-file <path>`
selects another global file. This is trusted startup configuration, separate
from credentials, and does not grant file tools access outside the project.
Missing global guidance is optional. Repository `AGENTS.md` applies to the whole
project. Nested `AGENTS.md` applies only to its directory and descendants,
including ignored paths and files that have not been created yet. A symlinked
project root uses its canonical project boundary.

Explicit user requests and built-in operating policy take precedence over
`AGENTS.md`. Within that guidance, repository instructions override global
instructions, and deeper directory instructions override ancestors within their
scope. `read_file` and `inspect_context` activate applicable nested guidance.
Every edit checks all affected paths, including both sides of a move. If rules
are new or changed since the model's last request, the edit fails before writing;
the next request includes those instructions for review before retrying. Workers
receive their own instruction-delivery state. Sources are refreshed for each
request and after resume; saved history cannot restore stale operating guidance.
New/clear and session switches reset activated nested scopes; subsequent reads
and edit preflights rediscover the applicable rules.
Ordinary file contents, web results, and worker reports stay in reference/tool
messages rather than becoming operating instructions.

`/context` and `inspect_context` show active instruction sources, scope,
precedence, built-in guidance, and a bounded inventory sample with truncation
metadata. `inspect_context` accepts an optional `path` to activate its scope.
Instruction files are limited to 64 KiB each and active guidance to 256 KiB;
loading errors stop continuation instead of truncating requirements. The normal
request context no longer repeats a complete symbol map or accumulated worker
reports. Relevant worker findings must be included in the delegated task.

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

Run the full suite from a terminal, CI, or agent-joe's Cargo tool:

```sh
cargo test --workspace --offline
```

The Cargo-cancellation, process-isolation, and sandboxed-toolchain tests probe
whether the runner permits creating process sandboxes before setting up their
fixtures. If a parent sandbox blocks this operation, these tests automatically
skip their bodies. Use `-- --nocapture` to see the skip reason; Rust's test harness
reports these runtime skips as passed. Fixtures that require hard links, named
pipes, writes to protected directories, or a loopback socket also skip the affected
checks when the runner denies permission. Other checks in the same test continue
to run. Unexpected probe or fixture errors still fail the tests.

Run the suite from a regular terminal or a CI host that permits creating process
sandboxes to exercise this coverage. The Cargo-cancellation test can be run
directly with:

```sh
cargo test -p actors runtime_test::interrupt_reaps_a_running_cargo_process_before_publishing_cancelled -- --exact
```

Linux process-isolation tests require `/usr/bin/bwrap` and host namespace support.
Container runners must allow nested mount/PID namespaces and mounting `/proc`.

Agent Cargo checks and tests share a persistent build cache at `target/.joe/build`
across workers and sessions. It is separate from terminal Cargo builds because the
sandbox uses its own Cargo home and dependency paths; sharing the terminal's build
directory would invalidate cached artifacts when switching between the two.
Concurrent agents use Cargo's build lock to reuse completed builds. The first agent
build populates this cache; later runs rebuild only what Cargo detects has changed.

## Keybindings

The TUI is similar to claude code and codex with one major difference. Vim bindings are foced upon you.

## Tools

Discovery tools are described above. Simple mode also exposes `apply_patch`,
`cargo_check`, `cargo_test`, and provider-supported web search. Worker mode uses
focused read, write, and validation workers. All modes retain the fixed project
policy and have no model-controlled shell.
