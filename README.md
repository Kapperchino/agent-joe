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

## Planning and interaction

Both default and `--simple` modes support the same interaction commands:

| Command | Behavior |
| --- | --- |
| `/plan` | Enter read-only planning mode and show saved steps |
| `/implement` | Return to implementation mode |
| `/questions` | Show pending questions and their answer commands |
| `/answer <id> choice <choice-id>` | Submit a listed choice |
| `/answer <id> text <answer>` | Submit permitted free text, preserving spaces and newlines |
| `/steer <correction>` | Cancel active work and queued follow-ups, then continue the corrected task after cleanup |

Change modes while idle, or interrupt and await cleanup first. Plan mode permits
discovery, reads, Git inspection, review, worktree listing, and read workers.
Dispatch rejects patches, undo, worktree mutations, every Cargo operation, and
write-worker launches, including follow-ups. Workers inherit the policy. Session
and plan storage still operates in plan mode; questions cannot change the fixed
project boundary or grant additional permissions.

The root uses `update_plan` to maintain 1–16 steps with stable IDs, dependencies,
acceptance criteria, and `pending`, `in_progress`, `completed`, or `blocked`
states. Only one step can be in progress. Dependencies must be acyclic and
completed before a dependent step starts. Completion requires a prior in-progress
state and recorded successful tool or answer evidence, with an explanation;
blocked steps require a reason. The runtime checks evidence provenance and
transitions. The model must assess whether that evidence establishes the stated
acceptance criteria. Failed tools cannot supply successful evidence, and reads
cannot establish that an unexecuted test passed.

The plan and a bounded catalog of evidence IDs appear in request context outside
operating instructions. Updates must match both the plan revision and the current
requirements revision. New user input or an answer marks an existing plan as
needing review: the model must reconcile it before further edits, Cargo, or final
completion. Completed steps must be reopened after requirements change. Plans
survive compaction, resume, and conversation forks.

`request_user_input` asks one question with an unused ID, a prompt, a required
flag, up to six named choices, and optional free text. At most eight questions
can be pending, with at most 256 question IDs per session. A required question
stops new tool dispatch, lets in-flight calls settle, cleans up owned workers and
processes, and waits for an explicit answer. Calls skipped in that batch are
recorded as unexecuted and are never replayed. The last required answer resumes
continuation or the next queued input. Optional questions allow independent work
and can remain pending after a turn finishes. Question tools do not wait for an
independent worker's filesystem lease.

Answers arriving during tools are committed immediately and inserted into the
transcript after the complete tool exchange. Invalid choices, duplicate answers,
and reused question IDs are rejected. Typing ordinary input never implicitly
answers a question: input submitted during active work or a required-question
wait stays in the FIFO follow-up queue. Use `/steer` for corrections to active
work. Cancellation preserves accepted results and does not roll back completed
edits. A correction does not answer any still-required question.

Resume restores mode, plan, and pending questions without starting work. `/clear`
and `/new` start fresh implementation sessions; the previous state remains in
the archive. The TUI shows mode, plan progress, pending questions, queue size,
worker count, and the latest validation result. Worker streams do not replace
the root response. Existing Vim editing, slash commands, and Ctrl-C cancellation
remain available.

## Tools

Both modes expose discovery, reading, `apply_patch`, the typed `cargo` tool below,
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
loads ignore files only within its allowed paths. `inspect_context`, Git, aggregate
review, worktree management, and Cargo require whole-project paths because their
effects or output cover the project. Scoped undo checks every affected path before
changing any file. Repository instructions are inherited independently of selected
file paths. A narrow edit worker can hand validation back to the root.

One worker owns workspace writes for its entire lifetime, including cleanup.
Additional writers and direct root edits/validation fail until it finishes.
Read workers can investigate concurrently; actual file tools retain the shared
read/write scheduler. Managed worktrees are available, but worker routing to them
is explicit and does not enable simultaneous writers in a shared workspace.

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
changed paths, recorded patch/undo edit IDs, Cargo attempts with their original
parameters and outcomes, final managed-process results, unresolved issues, artifact
references, elapsed time, and budget use. Cargo formatting and program execution
can change files without enumerating their paths; reports flag those effects for review.
Validation evidence distinguishes a failed tool attempt through its recorded
error; it does not claim that a process ran successfully. Large tool output uses
the existing session artifact storage. Parent completion requires retrieving all
new worker reports; missing reports fail the turn and cancel remaining work.
The parent must evaluate reported evidence against the completion criteria.

Interrupt, clear, shutdown and parent turn cleanup cancel owned workers and await
resource cleanup. Registrations and final reports persist with the parent session.
Resume exposes saved reports without restarting workers. A missing or inconsistent
final report becomes an interrupted result with explicitly uncertain effects and checks.
Debug stream files are written by the root; managed workers use their durable
session journals without writing debug files outside their allowed paths.

### Rust validation and execution

The `cargo` tool takes a required `operation` parameter:

| `operation` | Behavior |
| --- | --- |
| `check` | Compile selected Rust targets without running them |
| `test` | Run selected tests, optionally with an exact test filter |
| `fmt_check` | Check formatting |
| `fmt` | Apply formatting |
| `clippy` | Run Clippy, optionally denying warnings |
| `run` | Run a finite named binary or example |
| `start` | Start a managed named binary or example |
| `poll` | Read status and incremental output using a process ID |
| `stop` | Stop the microVM and all guest processes, reap the launcher, and return output |

Both root modes and write workers support all operations. Delegated workers
inherit the selected tools from their parent; the specialized validation worker
excludes `fmt`. Each worker's schema lists its allowed operations, and disallowed
requests fail before execution.

Build commands accept `workspace` or `package`, `features`, `all_features`,
`no_default_features`, `release`, and a built-in `target_triple`. `target` selects
`{ "kind": "lib" }`, `all`, `tests`, or a named `test`, `example`, or `bin`.
Runs require one named example or binary. Formatting accepts workspace/package
selection. Invalid combinations, unknown fields, paths used as target triples,
and option-like selectors fail before execution.

A focused regression request looks like:

```json
{
  "operation": "test",
  "package": "my-crate",
  "features": ["regression"],
  "target": { "kind": "test", "name": "integration" },
  "test_name": "regression_case",
  "exact": true
}
```

The `test` operation also accepts `show_output`; `clippy` accepts `deny_warnings`.
`include_warnings` remains accepted for compatibility, and structured diagnostics
always retain warnings. Start with relevant packages, features and tests before
broader checks. Workers can add focused regression tests when behavior warrants
coverage and report requested checks, executed checks, failures and limitations.
Compilation alone does not establish behavioral correctness.

The `run` and `start` operations accept literal `args` after Cargo's `--`. No shell expansion occurs.
`environment` contains additions to the clean sandbox environment: only
`RUST_LOG`, `RUST_BACKTRACE`, `NO_COLOR`, and uppercase `JOE_RUN_*` names are
allowed. Loader, toolchain, Cargo, home-directory and network settings cannot be
overridden. There are at most 64 feature names, 64 program arguments and 16
environment entries. Arguments and environment values reject control characters
and have a 4096-byte individual limit; the complete command and environment must
fit within 2048 serialized JSON bytes. Omitted options keep Cargo's default
selection. No credentials or other host environment values are inherited.

Commands run offline in a fresh libkrun Linux microVM on both macOS and Linux.
Joe automatically prepares and caches the sandbox on first use, including the
Linux guest and crates.io dependencies. No sandbox setup is required.
macOS also builds and executes Linux binaries, using the guest's Rust toolchain.
The project appears at `/workspace` inside the guest, and guest build artifacts
use `target/.joe/linux/build`. `timeout_seconds` is
1–300, defaulting to 300, including build and program execution. Each output
stream retains at most 16 MiB; exceeding the limit stops the process and reports
`output_limit` with captured output. Guest commands cannot access the host network
or localhost servers. Missing isolation or missing offline dependencies
produce failures rather than executing outside the sandbox.

Results are JSON on success and failure, with the requested executable, argument
vector, environment additions, workspace, starting revision where a workspace
lease applies, process status, exit code, elapsed milliseconds, compiler diagnostics, stdout and stderr.
Signals and launch failures can have no exit code. Large streams and diagnostics
have full session artifact references alongside bounded previews; use
`read_artifact` to retrieve them. Non-JSON output and stderr-only Cargo startup
errors are preserved. Every validation runs afresh (`reused: false`); Joe does
not cache validation results. Cargo can still reuse its own build artifacts.

Managed process IDs belong to the active turn. At most eight targets can be
started in a turn, and a running target blocks edits and other Cargo commands
across workers until it exits or is stopped. Reads and process controls remain
available. To receive incremental text, pass the previous `stdout.next_offset`
and `stderr.next_offset` in `offsets`; offsets count UTF-8 bytes in the returned
text. Omit offsets to retrieve all retained output. A paged artifact contains
the output segment starting at its stream's `offset`.

Turn completion, interrupt, clear and shutdown cancel managed processes and await
cleanup. Terminal status and retained output are journaled to the owning session,
including when no final poll occurs. Resume reports saved process evidence;
process IDs are never reattached or relaunched after restart. If a crash prevented
completion from being recorded, the outcome remains explicitly unknown. The
VMM is terminated along with every guest process, including deliberately detached
descendants.

## Git and task review

Both modes expose `git` and `review_changes`. `git` accepts `operation` values
`status`, `diff`, `show`, and `log`; diff targets are `staged`, `unstaged`, or
`head`. Optional paths are literal workspace paths. Show and log accept a commit
ID or a simple reference such as `HEAD`, `main`, or `HEAD~2`. Log defaults to 20
commits and caps at 100. The implementation uses libgit2 in process with SSH/HTTPS features disabled.
It does not launch Git or a shell or expose network operations. It does not execute
pagers, hooks, filters, external diff programs, or text converters. Inspection
never writes the index.

The first task turn captures tracked and nonignored untracked file contents,
ordinary permissions, HEAD, index blob IDs/modes/flags, and staged/unstaged differences. That baseline belongs
to the session and survives restart. Workers contribute to their parent's edit
journal. New sessions and conversation forks acquire their own baseline and edit
ownership; `/fork` continues to share the same filesystem.

`apply_patch` checks every operation, destination, and hunk before replacing any
file. Add and move destinations must be absent. A path may occur only once in a
patch, and overlapping parent/child paths are rejected. Reads remember the file
version: a concurrent edit requires a fresh read before another patch can use it.
Replacement files are staged beside their destinations, flushed, and rechecked
immediately before replacement. The LMDB journal records intent, each applied path, and any in-flight path whose
completion is uncertain. A failure can leave a partial patch; it reports the edit ID and completed
paths instead of promising a transaction across files. Interrupted intent remains
available for inspection and is never automatically replayed.

Use `/diff` for the aggregate review in the TUI. The `review_changes` tool returns
structured baseline/current Git status, staged and unstaged/untracked differences,
task diffs, individual Joe diffs, edit IDs, and external-change attribution. Edits
to ignored files still appear through the journal. Workers and the root agent are
instructed to inspect this review before claiming completion. Large tool reviews
use the existing session artifact mechanism.

`undo_changes` accepts an `edit_id`; `/undo <edit-id>` performs the same guarded
reversal while idle. It accepts only fully applied Joe edits for the current task.
Every file must still match the recorded result, including permissions or absence.
A conflict rejects the preflight, preserves user work, and leaves the index alone.
Cargo formatting and running programs can change files outside the edit journal.
Review includes those changes, but undo accepts only recorded edits whose results
still match.

## Managed worktrees

The root agent exposes `worktree` with `create`, `list`, `integrate`, and `remove`.
Creation requires an explicit `base` revision. `dirty_source` defaults to `reject`;
`base_only` explicitly starts from that commit without copying a dirty source
index, working tree, or untracked files. Joe generates the branch and directory
under `.joe-worktrees/<id>`, records their identity, and returns the path. A separate
Joe instance can be started in that directory for an isolated task. Parallel
writer scheduling remains the responsibility of worker coordination.

Integration copies the isolated file changes through the same preflight and
journal as ordinary edits. Source HEAD must still equal the chosen base, affected
source files must match their expected contents, and affected index entries must
be unchanged. It preserves the source index and reports conflicts instead of
merging over user work. Worktree edits must be committed or unstaged; staged
worktree changes are retained for explicit resolution. This integrates file
changes, not commit history.

Removal accepts only a recorded worktree whose files and HEAD still match its
initial base or last integrated result, with a clean index. Untracked or ignored
files, private session data, and changed branches prevent cleanup. Partial
creation/removal failures retain their management record for inspection. Worktree
mutations require the original repository root; an existing linked worktree can
inspect its verified shared control metadata but cannot mutate metadata outside
its fixed project root.

Git control paths must contain ordinary files and directories. Symlinks, hard
links, external object alternates, configuration includes, bare repositories,
submodules, and non-UTF-8 paths are unsupported. Global/system configuration is
disabled, and operational repository configuration is replaced with an empty
configuration after repository identity is checked. Git discovery uses project
`.gitignore` files, independently of discovery-only `.ignore` rules, and bounds
visits to 250,000 entries and paths to 32 MiB. Individual working files cap at
16 MiB; Git text output caps at 32 MiB; baseline content, serialized journals, and
aggregate tool reviews cap at 64 MiB. Limits fail explicitly. Existing session map
limits still apply. Cargo's macOS host policy permits fresh Git fixtures only inside
the command's private temporary directory; existing repository metadata remains
protected.
