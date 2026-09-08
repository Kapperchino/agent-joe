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

Both modes expose discovery, reading, `apply_patch`, `cargo_check`, `cargo_test`,
and provider-supported web search directly. `--simple` disables delegation.
All modes retain the fixed project boundary and have no model-controlled shell.

Default mode also provides `start_worker` and `worker_status`. A worker request
contains an objective, additional constraints, allowed tools and project-relative
paths, selected context or artifact references, completion criteria, and budgets.
Tool/path lists use newline-separated values; `.` allows the whole project.
Parent/user requirements and scoped repository instructions are inherited.
Required constraints are never silently truncated to fit a handoff.

`worker_status` supports `list`, `status`, `wait`, `cancel`, `cleanup`, and
`follow_up`. Waits return after at most 60 seconds. Follow-ups launch a new task
from a completed worker's report with the original constraints, permissions,
criteria and budgets. Cancel and wait before replacing an active task.
`gather_context`, `make_changes`, and `validate_rust` remain convenience tools:
they use fixed tool sets, allow the project, and await a bounded structured report.

Workers receive only their selected tools. File access intersects the parent's
policy, including read-only grants and protected paths. Ancestor directories can
be traversed for discovery; file contents remain restricted. Scoped discovery
loads ignore files only within its allowed paths. `inspect_context` and executable
operations require whole-project paths because their effects or output cover the
project. Repository instructions are inherited independently of selected file paths.
A narrow edit worker can hand validation back to the root.

One worker owns workspace writes for its entire lifetime, including cleanup.
Additional writers and direct root edits/validation fail until it finishes.
Read workers can investigate concurrently; actual file tools retain the shared
read/write scheduler. Worktree isolation and simultaneous writers belong to M8.

| Limit | Default | Maximum |
| --- | --- | --- |
| Active workers | 4 | 4 |
| Delegation depth | 1 | 1 |
| Worker starts per session, including follow-ups | 32 | 32 |
| Allocated worker tokens per session | 2,000,000 | 2,000,000 |
| Tokens per worker | 120,000 | 500,000 |
| Seconds per worker, before cleanup | 180 | 300 |
| Provider requests per worker | 16 | 32 |
| Tool calls per worker | 128 | 128 |

Token accounting conservatively reserves estimated request input and at most
4096 output tokens per request. Provider-reported consumption is recorded and
also stops over-budget workers. Estimates vary by provider tokenizer; reported
usage can exceed an estimate before the runtime receives it. Workers cannot
start unbudgeted context compaction. Cleanup never refunds allocated budgets;
session resume and forks retain saved allocations. Handoffs are limited to 64 KiB
of request data and 128 KiB including inherited requirements.

Reports contain lifecycle status, a concise explanation, confirmed and possibly
changed paths, validation tool attempts with their original parameters and
outcomes, unresolved issues, artifact references, elapsed time, and budget use.
Validation evidence distinguishes a failed tool attempt through its recorded
error; it does not claim that a process ran successfully. Large tool output uses
the existing session artifact storage. Parent completion requires retrieving all
new worker reports; missing reports fail the turn and cancel remaining work.
The parent must evaluate reported evidence against the completion criteria.

Interrupt, clear, shutdown and parent turn cleanup cancel owned workers and await
resource cleanup. Registrations and final reports persist with the parent session.
Resume exposes saved reports without restarting workers. A missing final report
becomes an interrupted result with explicitly uncertain effects and checks.
Debug stream files are written by the root; managed workers use their durable
session journals without writing debug files outside their allowed paths.

M7 currently builds on `cargo_check` and `cargo_test`. M6's expanded typed Cargo
operations and managed processes still need integration and validation; workers
inherit the direct tools supplied by their parent, so M6 can extend that surface.
