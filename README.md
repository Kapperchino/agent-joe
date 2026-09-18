# Joe Code

```text
            ▄▖▗▄ ▗▖
       ▄▄▟█████████▟█▖▄▖
     ▄▄████████████████▙▄▖
   ▗▄▟███████████████████▙▄▖
  ▄▄███████████████████████▄▖
  ▐█████████ ◕ ██ ◕ ████████
▗▟██████████▄▄▄██▄▄▄█████████▙▖
 ▜█▖▜▞▜██╭────╮╭────╮████▘▟▘▟▛
  ▝▀▄▝ ▝▀▜██▛▀     ███▀▘   ▟▘
     ▘    ▝▀█▛▘ ▝▀▀▀▘     ▝
```

**Small claws. Big ideas.** Ferris is Joe Code's mascot and your Rust coding companion.
The Unicode artwork follows [Karen Rustad Tölva’s original Ferris](https://rustacean.net/):
a spiky orange shell, black eyes with white highlights, and low, inward-facing claws.
The terminal pairs Ferris with warm charcoal surfaces and cream text.
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

Codex login requests use a stable session header for cache affinity and retain
server routing state only within the current turn. Token totals include cached
input; `--debug` records cached and uncached usage separately in `logs/err.log`.
Large tool outputs are archived, with artifact reads supporting up to 32 KB per
call. Review output refers to identical diffs with `same_as` JSON pointers.
The [token usage investigation](docs/token-usage.md) records the measured causes
and the comparison with Codex.

## Build & Run

```sh
cargo build --release
cargo run --release -p turbo-code
```

The default build compiles Joe and the `joe-sandbox` binary from the `sandbox`
package. Keep both executables in the same directory when installing Joe. Other
applications using the sandbox crate can set `JOE_SANDBOX_LAUNCHER` to the launcher
path. Workspace checks and tests include the launcher.

All session worktrees in a Joe project share one running sandbox VM. Switching,
restoring, or merging sessions does not restart it. Each command mounts only its
selected worktree at `/workspace`, hides the project export, and gets private
mount, PID, IPC, and network namespaces plus temporary storage. Timeouts and turn
cancellation stop its descendants without shutting down the VM. Workspace hard
links are checked before every command, and each command receives fresh
protection mounts for read-only and hidden paths.

Rust builds use a pinned, checksum-verified sccache with a persistent 10 GiB local
cache under the system cache directory at `agent-joe/sandbox/compiler-cache-v1`.
The guest image and compiler cache survive VM shutdown, application restarts,
and session/worktree deletion. Cargo target directories remain worktree-local;
unchanged cacheable compilations can be reused across worktrees, while linking
and build scripts may still run. Incremental compilation is disabled for sccache.
Commands acquire an exclusive cache lease, including across Joe processes, and
run a private sccache server that stops before the lease is released. Commands
therefore run serially, including managed programs, rather than racing multiple
servers against sccache's local storage. Cancelling a waiting command releases
its wait without stopping the shared VM.
Cargo timeouts default to 30 minutes and can be set between
1 and 3600 seconds with `timeout_seconds`.

Startup prepares the Linux guest image, guest init program,
and native components, then caches a copy of the built launcher outside the
workspace. On macOS, this copy is signed with hypervisor entitlements. Setup needs
network access and a C compiler, plus Rust's linker tools on macOS and `make` on
Linux. It does not invoke Cargo or compile the Rust launcher.

Cargo operations automatically download missing crates.io dependencies on the
host, verify their checksums, and cache them for the sandbox. This also works
without a lockfile and after dependencies change. Builds, build scripts, tests,
and programs run inside the sandbox without network access. Git dependencies and
custom registries are not downloaded automatically.

## Answering questions

Pending questions have a TUI picker that opens automatically when the message
editor is idle and empty. Press `?` in normal mode or run `/questions` to reopen
it. Use `↑`/`↓` or `j`/`k` to select an answer and `Enter` to submit it; no question
or choice IDs need to be typed. `Tab` and `Shift+Tab` switch between pending
questions. `PgUp`/`PgDn` scroll long prompts.

When free text is allowed, choose **Other (type an answer)**, or type directly for
text-only questions. `Enter` submits, `Ctrl+n` inserts a newline, and `Ctrl+u`
clears the answer. `Esc` returns from text entry to the choices or dismisses the
picker without answering; message drafts are preserved. Required questions remain
pending until answered. Manual `/answer <id> choice <id>` and
`/answer <id> text <answer>` commands remain available.

## Session storage

In Git repositories, each interactive session uses its own branch,
`joe/session/<session-id>`, checked out under `.joe-worktrees/<session-id>/`.
New sessions start from the committed local `main` branch; existing edits in the
original checkout stay there. A committed `main` branch is required. Workers use
their parent session's worktree. Projects without Git continue using their
original directory.

After a task completes successfully, Joe commits the session's changes and asks
whether to merge them into `main`. Select the merge or keep choice in the question
picker and press `Enter` (or use `/answer <question-id> choice <choice>`). Only explicit
acceptance updates `main`; ending a session, cancelling a task, or starting a new
session does not merge it. Starting another task invalidates the previous merge
question. Approval covers the commit shown in the question and any merge conflict
resolution; a new task requires a fresh question.

Merges preserve concurrent changes through a three-way merge. If conflicts arise
after approval, Joe resolves them in the session worktree, validates the result,
and retries the merge automatically without asking for approval again. `main`
stays unchanged until resolution succeeds; unresolved conflict markers block the
merge. Interrupting resolution stops the automatic merge. Local edits in the
`main` checkout or `main` checked out in another worktree stop the merge and leave
the session available for recovery. Resolve the blocker and retry the answer, or
complete another task to prepare a fresh merge proposal.
After merging, Joe stops the session sandbox and deletes the session branch and
the entire worktree directory, including ignored files and build caches. Unmerged
commits, uncommitted source changes, and pending Git operations block cleanup.
Session history stays in the original project. `/resume` or another task creates
a fresh worktree from current `main` after cleanup; `/fork` creates a separate
worktree including the current session's edits.

Use `/prune` to remove inactive session worktrees whose commits are already merged
into `main` and which have no local changes or pending Git operations. Use
`/prune --force` (or `/prune -f`) to also permanently discard worktrees with
unmerged commits, local changes, or pending Git operations. Pruning deletes their
branches and entire worktree directories, including ignored files and build
caches; forced pruning also discards untracked files. It does not merge anything
into `main`. Both modes skip the current session and sessions open in another
actor or process. Locked worktrees or changed Git identities are retained and
reported individually, even with `--force`.
Pruning requires implementation mode and an idle turn. It operates on saved
sessions in the current project's session namespace, not arbitrary Git worktrees.
Conversation history is retained, but `/resume` starts a fresh worktree from
current `main`; it does not restore the discarded changes or merge approval.

Session databases have a 100 GiB map limit and rotate before the next write once
usage reaches 90 GiB. The map reserves address space; disk space grows with the
stored data. A write that exceeds the remaining space is aborted and retried once
in a new database. This retries persistence, not the tool that produced the result.

The current database accepts writes. The previous database remains available to
session listing, search, and resume. Resuming an old session moves its conversation
into the current database. Active conversations, including their workers, event
history, ownership, and output artifacts, carry forward across rotations.

On the next rotation, the older database is compressed with Zstandard and verified
before its original files are removed. Archived sessions no longer appear in search
or the session picker and cannot be resumed directly. The current and previous
databases remain searchable, with duplicate sessions shown only once.

Storage stays under `.turbo-code/<session-namespace>/`. Existing `data.mdb` files
are adopted as generation zero. Later databases use `generation-*` directories;
`generations.json` identifies the current and previous generations. Archives are
named `archive-*.mdb.zst` and contain the complete LMDB data file. Rotation is
serialized across processes, and interrupted retirement is completed on restart.
Compression can take time and needs temporary space for the new database and
archive. If active conversations alone fill the next database, rotation stops and
retains the existing databases.

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

Sandbox integration tests require a host that can start a VM. On Linux, the test
helper checks access to `/dev/kvm` before looking for the launcher or provisioning
the runtime. Inside Joe's guest, KVM is unavailable, so these tests skip with a
diagnostic; other tests still run. Set `JOE_SANDBOX_REQUIRED=1` on the host to make
unavailable sandbox support fail the test run. A package-only run such as
`cargo test -p actors --lib` also needs a built launcher: run
`cargo build -p sandbox --bin joe-sandbox` first.

## Keybindings

The TUI is similar to claude code and codex with one major difference. Vim bindings are foced upon you.
