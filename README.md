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
`inspect_context`. Simple mode also exposes `apply_patch`, typed Cargo tools,
and provider-supported web search. Worker mode uses focused read, write, and
validation workers. All modes retain the fixed project policy and have no
model-controlled shell.

### Rust validation and execution

| Tool | Operation |
| --- | --- |
| `cargo_check` | Compile selected Rust targets without running them |
| `cargo_test` | Run selected tests, optionally with an exact test filter |
| `cargo_fmt_check` | Check formatting |
| `cargo_fmt` | Apply formatting; available to simple and write workers |
| `cargo_clippy` | Run Clippy, optionally denying warnings |
| `cargo_run` | Run a finite named binary or example |
| `cargo_start` | Start a managed named binary or example |
| `process_poll` | Read status and incremental output using a process ID |
| `process_stop` | Stop the process group, reap its leader, and return output |

Build commands accept `workspace` or `package`, `features`, `all_features`,
`no_default_features`, `release`, and a built-in `target_triple`. `target` selects
`{ "kind": "lib" }`, `all`, `tests`, or a named `test`, `example`, or `bin`.
Runs require one named example or binary. Formatting accepts workspace/package
selection. Invalid combinations, unknown fields, paths used as target triples,
and option-like selectors fail before execution.

A focused regression request looks like:

```json
{
  "package": "my-crate",
  "features": ["regression"],
  "target": { "kind": "test", "name": "integration" },
  "test_name": "regression_case",
  "exact": true
}
```

`cargo_test` also accepts `show_output`; `cargo_clippy` accepts `deny_warnings`.
`include_warnings` remains accepted for compatibility, and structured diagnostics
always retain warnings. Start with relevant packages, features and tests before
broader checks. Workers can add focused regression tests when behavior warrants
coverage and report requested checks, executed checks, failures and limitations.
Compilation alone does not establish behavioral correctness.

Run tools accept literal `args` after Cargo's `--`. No shell expansion occurs.
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
