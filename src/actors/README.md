# Actor composition

`actors` owns integration with session storage, provider tasks, tool execution,
file watchers, RPC replies, and UI delivery. `ActorState` assembles these adapters
with independently compiled state components.

`ActorState` coordinates turn effects and actor lifecycle. Its components own
the following responsibilities:

| Type | Responsibility |
| --- | --- |
| `session::state::SessionState` | Owns conversation, interaction, persistence, and merge state; lends them to the existing domain controllers |
| `session::turn::SessionTurn` | Coordinates turn startup, answer commits, merge readiness, and persisted tool batches |
| `session::changes::SessionChanges` | Starts change tracking and executes authorized diff and undo commands |
| `ProviderContext` | Builds and launches requests, captures immutable snapshots, and reviews completion against plans and pending workers |
| `ProviderStream` | Owns stream accumulation, logging, usage, notifications, and provider event translation |
| `ProviderSession` | Persists provider usage and context updates, installs committed checkpoints, and registers immutable context workers |
| `ActorServices` | Resolves tools and applies tool context updates with failure handling |
| `ActorMode` | Configures startup tools, request mode, and event reporting |
| `EventReporter` | Delivers notifications and formats command results through one path |

Session activation replaces `SessionState` as a unit and resets the provider
stream with the restored usage. Stream actions request persistence or context
commits from the actor before the turn machine advances.
`SessionPersistence` reports terminal storage failures before merge or worker
completion can proceed. Provider input failures drain the previous request and
use the same task completion path as provider responses. Tool launchers report
rejected batches without running tools.

| Crate | Owns | Boundary |
| --- | --- | --- |
| `conversation` | Transcript, deferred input, checkpoints, context budgeting | Restores from `SavedConversation`; produces provider request data |
| `interaction` | Plan and question transitions, access checks, commands, tool policy | `InteractionControl` commits through `InteractionPersistence` before publishing state |
| `response-stream` | Stream accumulation, usage, completed response items | Produces `StreamUpdate` notifications; performs no reporting or file I/O |
| `turn-engine` | Turn lifecycle, tool batches, retries, cleanup sequencing | Consumes events and produces effects; worker replies carry request IDs |
| `worker-registry` | Worker lifecycle, request limits, accounting, evidence, reports | Accepts cancellation tokens, token usage updates, and supplied report evidence |
| `workspace-access` | Reader limits, write leases, workspace revisions | Acquires leases using tool effects and execution scopes |
| `merge-workflow` | Merge approval, Git execution, commit descriptions, conflict resolution, recovery | `SessionMerge` uses `MergePersistence` and returns relocation or resolution actions |
| `session` | Session storage, artifacts, persistence, activation, transitions, commands | Owns `SessionRuntime`; implements interaction and merge persistence interfaces |

These eight crates do not depend on `actors`. `session` composes the conversation,
interaction, turn, worker, workspace, and merge components. Merge execution uses
interaction control and the response components. Shared values live
in `common-models`, `clients::response`, and the existing tool and execution
contracts. Artifact references live in `utils::artifacts` and are shared by Cargo
output, session storage, and worker reports. Actor lifecycle coordination belongs
in `actors`.

Session activation maps stored snapshots into each component's input type. The
session controllers record interaction events before installing the resulting
state. The actor delivers stream notifications, resolves worker reply IDs, and
executes turn effects.
Context is owned directly by the actor and cloned when preparing tool execution;
it does not contain the runtime or session store.

Worker launch, provider accounting, and session evidence retrieval remain actor
adapters. The registry neither starts actors nor opens sessions. Its budget
accepts `RequestReservation` and `UsageUpdate`; the provider adapter estimates
request tokens and translates provider usage. `WorkerSession` filters inherited
artifacts and supplies `StoredWorkerEvidence` after cleanup. Reports retain
observed tool evidence when session evidence cannot be retrieved.

Session control, interaction control, merge execution, and workspace transitions
take explicit component references and have no dependency on `ActorState`.
`session::persistence::SessionPersistence` records events and reports storage
failures. `interaction::control::InteractionControl` publishes only committed
state. Session commands return reports, clear requests, or prepared activations.
Merge execution returns a
workspace relocation or conflict-resolution request when actor work is needed.
`states::actor_state` applies these results, synchronizes the question gate, and
coordinates actor lifecycle operations. The actor maps its runtime into
`SessionRuntime` and `MergeEnvironment`, attaches worker sessions, and implements
`common_models::tui_models::EventSink` for UI delivery.

Merge approval does not inspect the turn machine, runtime, or session store.
`merge_workflow::execution::SessionMerge` maps activity and persistence state into
`MergeReadiness`, records approval transitions, and performs Git operations.
The workflow decides whether
a proposal needs approval and whether a saved question must be restored. Saved
resolutions resume paused, preserving the existing storage format.

Callers import domain types from their owning crates. Actor modules contain the
adapters that coordinate those types. Component unit tests live in their owning
crates; actor integration tests exercise storage, streaming, worker execution,
and session switching together.

## Composable agent workflows

`run_workflow` runs an ordered list of typed steps in the default root agent.
Agent definitions are reusable profiles, separate from step objectives. Each
agent step starts a distinct bounded worker, waits for its report, and collects
that report before starting the next step. Workers share the session workspace;
writers do not overlap within a pipeline.

The following is the tool input for a three-agent **code → standards rewrite →
simplify** pipeline. Replace the shared context with the actual requested feature
and selected source information.

```json
{
  "context": "Implement the requested feature while preserving the public API.",
  "agents": [
    {
      "name": "coder",
      "instructions": "Implement the behavior and focused regression tests.",
      "allowed_tools": "find_files\nlist_directory\nknowledge\ngrep\napply_patch\ncargo\ngit\nreview_changes",
      "allowed_paths": ".",
      "seconds": 1800
    },
    {
      "name": "standards",
      "instructions": "Read scoped AGENTS.md guidance and rewrite the implementation to repository code standards without changing behavior.",
      "allowed_tools": "find_files\nlist_directory\nknowledge\ngrep\napply_patch\ncargo\ngit\nreview_changes",
      "allowed_paths": ".",
      "seconds": 1800
    },
    {
      "name": "simplifier",
      "instructions": "Simplify the implementation without changing behavior; retain regression coverage and run relevant checks.",
      "allowed_tools": "find_files\nlist_directory\nknowledge\ngrep\napply_patch\ncargo\ngit\nreview_changes",
      "allowed_paths": ".",
      "seconds": 1800
    }
  ],
  "steps": [
    {
      "kind": "agent",
      "id": "code",
      "agent": "coder",
      "objective": "Implement the requested feature",
      "completion_criteria": "Behavior implemented with focused regression coverage"
    },
    {
      "kind": "agent",
      "id": "rewrite",
      "agent": "standards",
      "objective": "Rewrite the implementation to repository standards"
    },
    {
      "kind": "agent",
      "id": "simplify",
      "agent": "simplifier",
      "objective": "Simplify the implementation and validate the final behavior"
    }
  ]
}
```

The existing convenience workflows are built-in profiles. No custom agent
definitions are needed to compose them:

```json
{
  "context": "Fix the reported bug while preserving unrelated changes.",
  "steps": [
    {
      "kind": "agent",
      "id": "investigate",
      "agent": "gather_context",
      "objective": "Locate the relevant implementation and regression coverage"
    },
    {
      "kind": "agent",
      "id": "implement",
      "agent": "make_changes",
      "objective": "Fix the bug and add a focused regression test"
    },
    {
      "kind": "agent",
      "id": "validate",
      "agent": "validate_rust",
      "objective": "Run the relevant Cargo tests and report their actual results"
    }
  ]
}
```

`gather_context`, `make_changes`, and `validate_rust` still accept their original
`context` argument and return a `WorkerReport` when called individually. They now
use the same runner as one-step workflows. Their tool/path allowances and default
1800-second worker deadlines are unchanged.

To add a phase, append an agent step referencing a built-in or custom profile.
The same profile can be referenced by multiple steps; each reference starts a
new worker. To insert reference material without starting an agent, use a
context step:

```json
{"kind":"context","id":"standards","content":"Preserve the public API"}
```

Agent steps also accept their own `context` and `completion_criteria`. Step IDs
and custom agent names must be nonempty and
unique; built-in names cannot be replaced.

Every agent receives shared context, its own step context, and summaries of all
earlier steps. Summaries include findings, changed files, unresolved issues and
artifact references. Full worker reports, including observed validation evidence,
remain in the final `WorkflowReport`. A noncompleted worker, launch failure or
oversized handoff stops the pipeline and marks later steps skipped. Completed
workers may still report limitations or failed validation: workflow completion
does **not** establish that tests passed. The root agent must assess the reports
and review the aggregate changes before finishing.

Configuration supports 1–16 steps and at most 16 custom profiles. Configuration
and each worker handoff are limited to 64 KiB; oversized handoffs fail rather than
silently discard earlier findings. Custom deadlines are 1–3600 seconds, defaulting
to 1800 per worker. Existing worker concurrency, start limits, cancellation,
permissions and project boundaries still apply. Tool names and paths are
newline-separated strings. Whole-project tools such as Cargo and Git require
`allowed_paths: "."`; narrower scopes can use restricted file tools. Agents cannot
delegate further. Read-only pipelines remain usable in plan mode, but pipelines
with editing or Cargo steps require implementation mode.

### Extension boundaries

`worker_registry::workflow` owns the configuration, validated ordered steps,
handoff construction and running/stopped state machine. `Workflow::run` accepts
an asynchronous worker executor, so its ordering and failure behavior can be
tested without actors or providers. `actors::tools::delegated` supplies the
existing authorized worker launcher, writer exclusion and report collection.
The `run_workflow` tool exposes the configuration with a structured schema and
derives its effect and execution deadline from the compiled steps.

New agent phases are configuration-only changes. A new non-agent step behavior
belongs in `StepInput`, its validated `StepAction`, and the runner dispatch, with
a corresponding report variant and regression coverage. No new lifecycle or
worker-launch implementation is needed. Pipelines are sequential; they do not
add branching, parallel execution or automatic pipeline resumption.

## Repository knowledge integration

`common-models::knowledge` owns the validated semantic graph protocol.
`knowledge-indexer` is a library using pinned rust-analyzer crates directly. It
constructs a semantic database from captured Cargo manifests, source, and local
path dependencies without running Cargo, build scripts, or proc macros. There is
no executable or guest provisioning. `utils::knowledge` snapshots approved inputs,
owns cooperative preparation/cancellation, checks exact source coverage, and
fingerprints live inputs. `analysis::knowledge` assigns exact source ownership,
cuts a deterministic SCC/DFS forest to rendered request budgets, and builds bounded
symbol/path/relationship routing indexes.

`actors::knowledge` owns generation state (`Preparing`/`Ready`, with a separately
invalidatable freshness state). A preparation ticket atomically replaces graph
and worker registry entries only after all shards fit and the inputs remain current.
Failure restores the previous generation; concurrent clear prevents late publication.
All shard states reuse the frozen tool-free request executor without turning their
source into instructions. The estimator and executor build the same request.
Read-only questions never append conversation history or acquire tools.

The immutable registry retains compaction snapshots when replacing or clearing
knowledge. Knowledge admissions stay attached to queued messages until handled or
dropped, and a shared semaphore serializes provider requests across shards. Caller
cancellation and generation retirement drop in-flight requests. Workspace/model
fingerprints are checked at query and answer boundaries. Runtime activation and
shutdown clear the owner registry; relocation clears only repository knowledge.

Main, simple, read, and write workers expose `knowledge` as the unified context
tool. `read` accepts `file_path` and an optional one-based, end-exclusive `range`,
returning current line-numbered `content` and `related` context. Related context
includes routing context and immutable worker IDs when a prepared generation is
available, or an explicit unavailable reason without blocking the read. Reading
does not prepare knowledge or invoke a provider. `list` discovers immutable context
workers and `ask` accepts `worker_id` and `question`, including compaction snapshots.
Preparation is classified as `Validate` from its parsed input, `ask` as
`DelegateRead`, and other actions as `Read`. Delegated/restricted contexts can read
allowed files, but cannot query whole-repository knowledge or list/ask immutable
workers. No preparation occurs on startup.

Main and simple root workers share the actionable workflow in
`src/workers/resources/knowledge.md`. Nontrivial investigations and cross-module
changes check `status`, explicitly `prepare` absent or source-stale knowledge in
implementation mode, route with `search`/`inspect`, and `ask` relevant immutable
workers before choosing an implementation. Current reads confirm source before
edits; `list`/`ask` recover missing conversation details after compaction.
Single-file tasks can stay with focused discovery and reads. Plan mode cannot
prepare, and scoped children rely on parent-provided semantic context. Preparation
or query failures must be reported before falling back to ordinary discovery;
context answers do not replace behavioral validation. Source changes require
preparation again, while model/budget-only changes permit `repartition`. `clear`
intentionally retires repository knowledge without discarding snapshots.
