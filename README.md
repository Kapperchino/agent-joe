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

Discovery tools are described above. Simple mode also exposes `apply_patch`,
`cargo_check`, `cargo_test`, and provider-supported web search. Worker mode uses focused read, write, and validation
workers. All modes retain the fixed project policy and have no model-controlled shell.

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
