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

Discovery uses `find_files`, `list_directory`, `grep`, `read_file`, and
`inspect_context`. Simple mode also exposes `apply_patch`, the typed `cargo` tool,
and provider-supported web search. Worker mode uses focused read, write, and
validation workers. All modes retain the fixed project policy and have no
model-controlled shell.

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
| `stop` | Stop the process group, reap its leader, and return output |

Simple workers support every operation. Validation workers support all except
`fmt`; write workers support only `fmt` and delegate validation. Each worker's
schema lists its allowed operations, and disallowed requests fail before execution.

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

Commands run offline in the shared macOS/Linux sandbox. `timeout_seconds` is
1–300, defaulting to 300, including build and program execution. Each output
stream retains at most 16 MiB; exceeding the limit stops the process and reports
`output_limit` with captured output. Network access, including localhost
servers, remains unavailable. Missing isolation or missing offline dependencies
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
existing sandbox limit on guaranteeing termination of deliberately detached
descendants still applies.

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
limits still apply. Cargo's macOS sandbox permits fresh Git fixtures only inside
the command's private temporary directory; existing repository metadata remains
protected.
