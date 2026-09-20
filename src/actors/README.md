# Actor composition

`actors` owns integration with session storage, provider tasks, tool execution,
file watchers, RPC replies, and UI delivery. `ActorState` assembles these adapters
with independently compiled state components.

| Crate | Owns | Boundary |
| --- | --- | --- |
| `conversation` | Transcript, deferred input, checkpoints, context budgeting | Restores from `SavedConversation`; produces provider request data |
| `interaction` | Plan and question transitions, tool policy | Produces `InteractionEvent`; commits through a supplied recording function |
| `response-stream` | Stream accumulation, usage, completed response items | Produces `StreamUpdate` notifications; performs no reporting or file I/O |
| `turn-engine` | Turn lifecycle, tool batches, retries, cleanup sequencing | Consumes events and produces effects; worker replies carry request IDs |
| `worker-registry` | Worker lifecycle, request limits, accounting, evidence, reports | Accepts cancellation tokens, token usage updates, and supplied report evidence |
| `workspace-access` | Reader limits, write leases, workspace revisions | Acquires leases using tool effects and execution scopes |

These six crates do not depend on `actors` or on one another. Shared values live
in `common-models`, `clients::response`, and the existing tool and execution
contracts. Artifact references live in `utils::artifacts` and are shared by Cargo
output, session storage, and worker reports. Changes that coordinate components
belong in `actors`.

Session activation maps stored snapshots into each component's input type. The
actor records interaction events before installing the resulting state, delivers
stream notifications, resolves worker reply IDs, and executes turn effects.
Context is owned directly by the actor and cloned when preparing tool execution;
it does not contain the runtime or session store.

Worker launch, provider accounting, and session evidence retrieval remain actor
adapters. The registry neither starts actors nor opens sessions. Its budget
accepts `RequestReservation` and `UsageUpdate`; the provider adapter estimates
request tokens and translates provider usage. `WorkerSession` filters inherited
artifacts and supplies `StoredWorkerEvidence` after cleanup. Reports retain
observed tool evidence when session evidence cannot be retrieved.

Callers import domain types from their owning crates. Actor modules contain the
adapters that coordinate those types. Component unit tests live in their owning
crates; actor integration tests exercise storage, streaming, worker execution,
and session switching together.
