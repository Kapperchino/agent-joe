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

The root main/simple worker exposes `knowledge` plus `ask_immutable_worker`.
Preparation is classified as `Validate` from its parsed input; search, inspection,
status, repartitioning, and clear are `Read`. Delegated/restricted contexts cannot
query whole-repository knowledge. No preparation occurs on startup. See the root
README for profiles, ceilings, generation IDs, freshness behavior, direct-library
tests, and unavailable sysroot/external/generated-code coverage.
