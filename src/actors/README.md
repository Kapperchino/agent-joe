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

These four crates do not depend on `actors` or on one another. Shared values live
in `common-models`, `clients::response`, and the existing tool and execution
contracts. Changes that coordinate components belong in `actors`.

Session activation maps stored snapshots into each component's input type. The
actor records interaction events before installing the resulting state, delivers
stream notifications, resolves worker reply IDs, and executes turn effects.
Context is owned directly by the actor and cloned when preparing tool execution;
it does not contain the runtime or session store.

The small re-export modules retain existing import paths for callers. Component
unit tests live in their owning crates; actor integration tests exercise storage,
streaming, worker execution, and session switching together.
