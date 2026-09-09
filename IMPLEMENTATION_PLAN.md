# Agent Joe implementation plan

Updated: 2026-09-08. M1–M9 are complete, including combined M7/M8 integration
with the consolidated Cargo tool and M9 planning/user interaction. M10 remains planned.

Cover the ten gaps identified in the Codex comparison while keeping Joe a
Rust-focused agent with typed tools and no model-controlled shell.

**Scope and operating decisions**

- Preserve the TUI, Vim bindings, provider choices, and `--simple` option.
  Both agent modes share runtime services.
- Keep executable operations behind validated schemas and the shared sandbox.
  Cargo is the only production executable operation family; Git uses libgit2
  in process. Future tools must register typed operations without exposing shell
  text or arbitrary command strings.
- Keep model tools inside the fixed project boundary. Allowed operations run
  automatically; denied operations fail without permission prompts, escalation,
  or an unconfined fallback.
- Use macOS/Linux isolation for executable repository operations and in-process
  filesystem policy for file tools and passive analysis. Unavailable isolation
  disables executable operations while file tools remain available.
- Keep runtime, workspace, and session settings separate from credentials, with
  backward-compatible defaults.
- Use temporary workspaces, recorded fixtures, and local fake providers for
  deterministic tests. Live provider checks and task comparisons are opt-in.
- Measure correctness, preservation of existing changes, completion time, and
  token use. Feature completion alone does not establish better coding performance.

**Milestone status**

| Milestone | Current status | Result / comparison gap | Dependencies |
| --- | --- | --- | --- |
| M0 | Baseline established | Deterministic build/test foundation | — |
| M1 | Complete on main | Model state and instruction correctness / 1 | M0 |
| M2 | Complete on main | Turn lifecycle, cancellation, scheduling / 3 | M1 |
| M3 | Complete on main | Workspace policy and sandbox / 4 | M2 |
| M4 | Complete on main | Sessions, artifacts, compaction / 2 | M1–M3 |
| M5 | Complete on main | Discovery and scoped instructions / 5, 7 | M3–M4 |
| M6 | Complete on main | Typed Cargo validation and managed targets / 6 | M2–M5 |
| M7 | Complete | Direct work and bounded delegation / 8 | M4–M6; integration with M8 |
| M8 | Complete | Git, aggregate review, guarded undo, worktrees / 9 | M3–M6; worker integration with M7 |
| M9 | Complete | Plan mode, tracked steps, questions, steering / 10 | M4–M8 |
| M10 | Planned | Skills and controlled MCP integrations / 10 | M3–M5, M9 |

M7 and M8 share worker policy, workspace ownership, the parent change journal,
and durable session state. Managed worktrees do not enable simultaneous writers
without explicit routing and ownership.

**Shared architecture**

- `clients`: provider capabilities, request construction, native response items,
  stream decoding, and provider-specific compaction.
- `actors`: turn state machines, scheduling, sessions, context projection,
  compaction, and change coordination. M7 adds the bounded worker registry.
- `tools`: validated schemas, declared effects, discovery, patches, Cargo, Git,
  review, undo, and managed worktree adapters.
- `utils`: workspace policy, bounded filesystem operations, inventory, sandbox,
  processes, Git access, and change journals.
- `analysis`: passive Rust semantic context and scoped instructions, layered over
  the full workspace inventory.
- `common-models`, `commands`, and `app`: shared runtime IDs, progress/results,
  commands, and TUI interaction.

The model receives a bounded projection of the durable session. Full event
history and output artifacts remain stored separately. Native reasoning and
compaction items retain their provider identity and are replayed only to a
compatible route. Keep crate dependencies acyclic and domain validation in the
relevant Rust types.

**M1 — Model state and instruction correctness — complete**

- Operating instructions, workspace context, and user/delegated messages use
  separate request channels. Clearing history preserves operating configuration
  and refreshes context.
- Native reasoning, IDs, encrypted content, message phases, and Claude signatures
  survive continuation. Incompatible reasoning is rejected; optional request
  fields remain route-specific.
- Native JSON arguments, stable call IDs, and grouped results survive replay.
  Only complete, validated tool batches execute. Partial, malformed, failed, or
  truncated responses cannot trigger tool side effects.
- Deterministic tests cover provider mapping, streaming, tool cycles, history,
  continuation, and clear behavior.

Code: `src/clients/src/{llm,openai,openai_mappings,claude_mappings,sse}.rs`;
`src/actors/src/{batch,stream_processor}.rs`.

**M2 — Turn lifecycle, cancellation, and scheduling — complete**

- Both modes share typed turn states, stable operation IDs, owned tasks, and
  resource tracking. Stale/duplicate events cannot alter active history, and
  worker completion/failure resolves parents.
- Interrupt, clear, and shutdown cancel owned tasks and await cleanup. Process
  groups are signalled and leaders reaped. Blocking filesystem work finishes
  before releasing its lease; completed writes are not rolled back.
- User follow-ups queue FIFO. Reads have bounded concurrency; writes and
  validation use an exclusive workspace lease with revision tracking.
- Accepted results survive interruption. Unstarted calls are recorded as
  unexecuted, uncertain effects are never automatically replayed, and provider
  recovery uses structured failures and bounded retries.
- Tests cover whole turns, split SSE/UTF-8, premature EOF, concurrency,
  cancellation, recovery, and lifecycle races.

Code: `src/actors/src/{turn,turn_machine,turn_driver,scheduler,supervisor}.rs`;
`src/utils/src/execution.rs`.

**M3 — Workspace policy and sandbox — complete**

- Descriptor-based filesystem operations enforce the project boundary and reject
  traversal, redirected aliases, unsafe links, and special files. Ordinary writes
  replace sibling files atomically and preserve permissions. Ordinary file tools
  cannot access Joe storage or write repository control metadata.
- Analysis is passive with an in-memory symbol cache. Startup runs no Cargo,
  build scripts, proc macros, or sandbox probes; dependency resolution and macro
  expansion remain unavailable.
- The sealed `SandboxOperation` contract admits registered typed operations.
  M6 expanded Cargo beyond the original check/test surface.
- macOS uses Seatbelt; Linux uses Bubblewrap namespaces, dropped capabilities,
  and seccomp. Processes run offline with a clean environment, project-local
  writes, read-only toolchains/caches, and a fresh temporary directory that is
  removed after execution or cancellation.
- Existing hard links are accepted for process workspaces only when every alias
  stays inside the workspace with the same access permissions. Processes cannot
  create new hard links. Private temporary fixtures can create session storage
  and Git repositories without exposing existing project storage.

Code: `src/utils/src/{workspace,files,sandbox}.rs` and their submodules.

Boundary limits: Linux requires `/usr/bin/bwrap` and host namespace support.
Deliberately detached descendants remain confined, but guaranteed termination is
outside scope. macOS semaphore access relies on ordinary OS ownership and is not
scoped by workspace; read-only process information supports session ownership.
Trusted provider connections and user setup are outside the model tool surface.
Concurrent hostile host processes are outside the threat model.

**M4 — Sessions and context management — complete**

- Versioned LMDB events and snapshots commit atomically under protected
  `.turbo-code/sessions` storage. Ownership uses process identity and unique tokens;
  dead owners can be reclaimed without stale handles accessing a newer session.
  Credentials are not serialized.
- `/sessions`, `/resume [id]`, `/new`, `/clear`, `/fork`, and `/compact` are
  implemented. Resume revalidates workspace/provider identity and current
  instructions, restores the transcript, and waits for user input. Forks own
  independent conversation state while sharing the filesystem.
- Tool intent commits before execution and completion before reporting. Recovery
  matches every saved call with completed, unexecuted, or uncertain evidence and
  never launches saved operations. Storage errors stop automatic continuation.
- Large outputs become immutable artifacts in the completion transaction.
  `read_artifact` retrieves bounded UTF-8 pages. An indexed conversation membership
  table permits descendant-worker access and freezes a fork's inherited access.
- Requests prioritize instructions, verbatim requirements, pending questions,
  validation/failure evidence, recent complete exchanges, and response space.
  Automatic compaction starts at 80% of the input budget; `/compact` uses the
  same cancellable workflow.
- Public OpenAI uses native compaction; compatible routes can opt in. Other
  routes use a tool-free compaction worker. Native items remain intact. A
  checkpoint commits before continuation; failures preserve the previous context.
- Runtime flags configure budgets, compaction routing, and storage separately
  from credentials. The TUI separates request-context estimates from cumulative
  usage. Question/answer records persist; M9 provides their interactive flow.

Code: `src/actors/src/{session,session_control,context,compactor}.rs`,
`src/actors/src/session/*`, and `src/clients/src/compaction.rs`.

Limits: estimates use serialized UTF-8 bytes and framing rather than provider
tokenizers. Protected requirements/evidence that cannot fit cause a visible stop.
The Codex route reserves response space locally without sending an output-token
limit. Native state cannot fall back to text summaries. Compaction retains the
full archive and does not reclaim LMDB space.

**M5 — Repository discovery and instructions — complete**

- Deterministic inventory applies project-local nested `.gitignore`/`.ignore`
  rules independently of Rust symbols. `find_files`, filtered literal/regex
  `grep`, and paginated `list_directory` cover non-Rust, hidden, empty, and new
  files. Explicit allowed reads can access ignored files.
- Reads use current disk content and one-based line ranges with exclusive ends.
  Invalid ranges produce structured failures. Semantic queries hold their
  analysis lock so refreshes cannot cancel an active snapshot.
- Both root modes share watcher startup/shutdown. Edits refresh immediately;
  discovery and semantic reads refresh without waiting for notifications.
- Global, repository, and applicable nested `AGENTS.md` sources have explicit
  precedence, scope, and provenance. Rules refresh on requests. Patch preflight
  rejects unseen or changed guidance for source/destination paths before writing.
  Workers track instruction delivery independently.
- `/context` and `inspect_context` expose active sources and truncation. File
  contents stay in reference messages. Requests omit repeated symbol maps and
  accumulated worker reports.

Code: `src/utils/src/{inventory,discovery,text_search}.rs`,
`src/analysis/src/instructions.rs`, discovery tools, and
`src/actors/src/background_actors/file_actor.rs`.

**M6 — Typed validation and runnable Rust targets — complete**

- One `cargo` tool selects `check`, `test`, `fmt_check`, `fmt`, `clippy`,
  `run`, `start`, `poll`, or `stop`. Both root modes and write workers support all
  operations. Managed workers inherit selected tools from their parent; the specialized
  validation worker excludes `fmt`. Schemas and dispatch enforce tool permissions.
- Constructors validate workspace/package, features, profile, built-in target
  triple, target kind/name, and test filter selections. Runs require a named
  binary/example. Unknown fields, conflicting selectors, option injection, and
  custom target paths fail before execution.
- Arguments remain literal after `--`. Environment additions allow only
  `RUST_LOG`, `RUST_BACKTRACE`, `NO_COLOR`, and uppercase `JOE_RUN_*` names.
- Successes and failures retain JSON command details, workspace/revision, status,
  exit code, timing, diagnostics, and separate stdout/stderr. Large results use
  artifacts; managed output supports incremental UTF-8 byte offsets.
- Managed processes belong to the active turn. Running targets block edits and
  other Cargo commands across workers. Stop, interrupt, clear, completion, and
  shutdown await cleanup and journal final evidence, even without a final poll.
  Resume reports saved evidence without reattaching to PIDs or relaunching.
- Every validation runs afresh (`reused: false`); Cargo may reuse build artifacts.
  Worker prompts call for focused regressions, targeted checks, and explicit
  reporting of requested/executed checks, failures, and limitations.

Code: `src/utils/src/{cargo,process,sandbox,execution}.rs`,
`src/tools/src/cargo_tools.rs`, scheduler/session integration, and worker prompts.

**M7 — Worker coordination — complete**

Default mode supports direct work and bounded delegation through the shared
registry. `--simple` performs direct work without delegation.

- The default root shares the simple worker's direct tools and adds asynchronous
  worker start/control tools. Simple mode remains free of delegation.
- Typed requests carry objectives, inherited constraints, selected context,
  completion criteria, allowed tools/paths, and budgets. Dispatch and filesystem
  policy enforce restrictions; workers cannot delegate further.
- Registration and completion channels precede startup. Controls support
  list/status, bounded waits, cancellation, cleanup, and fresh follow-up tasks
  after completion. Four workers may be active, with one workspace writer whose
  ownership lasts through cleanup and excludes root writes.
- Per-worker token, time, request, and tool-call limits combine with session
  allocations. Workers cannot initiate unbudgeted compaction. Full-project
  operations, including Cargo execution, require full-project path access.
- Typed reports persist patch/undo edit IDs, uncertain effects, validation
  parameters/results, artifacts, unresolved issues, and usage. M6 managed-process evidence is retained
  after cleanup; formatting/program execution report effects with unknown paths.
- Parents must retrieve new reports before completing. Handoffs preserve selected
  context and inherited requirements. Resume marks unfinished workers interrupted
  without replay; forks retain reports/allocations with separate control scopes.
- Worker lifecycle and report collection use one state machine. Late updates cannot
  replace finished reports; recovery requires a matching terminal report and marks
  missing or inconsistent evidence interrupted. Path restrictions enforce their
  invariants in the restriction constructor.

Code: `src/actors/src/worker_registry.rs`,
`src/actors/src/worker_registry/*`,
`src/actors/src/tools/{start_worker,worker_status}.rs`, and
`src/actors/src/workers/task_worker.rs`.

Workers share the parent task baseline and journal. Root review/undo can act on
recorded worker edits after writer cleanup. Scoped workers cannot access
repository-wide Git, aggregate review, or worktree management; scoped undo
preflights every affected path. Cargo evidence uses the actual requested operation.
Follow-ups start fresh workers; changing an active task requires cancellation and
cleanup. Managed worktrees do not route workers automatically.

Acceptance: direct completion in both modes plus optional, observable, bounded,
cancellable delegation that preserves validation evidence and project policy.
Concurrent writers require explicit ownership or isolated workspaces.

**M8 — Git and complete change review — complete**

- Typed in-process Git status/diff/show/log use validated literal paths and
  restricted revisions. Network transports, executable helpers, and external
  configuration are disabled. Inspection does not write the index.
- Task baselines capture working files, permissions, HEAD, and index state.
  Durable edit records attribute worker changes to their parent and distinguish
  pre-existing and observed external changes. Resume retains ownership; new
  sessions and history forks acquire independent task ownership.
- Patches preflight every operation, destination, and hunk; reject stale content,
  occupied destinations, and overlapping paths; stage replacements; and recheck
  before replacement. Per-path journal progress records partial/uncertain outcomes.
  Multi-file patches are nontransactional and are never automatically replayed.
- `review_changes` and `/diff` aggregate task/Joe diffs and staged, unstaged,
  untracked, and journaled ignored-file changes. Root/write-worker prompts require
  review before claiming completion. Large reviews use artifacts.
- `undo_changes` and `/undo <edit-id>` reverse only fully applied recorded edits
  whose files still match the recorded result, including permissions or absence.
  Conflicts preserve user work and the index. Cargo/program effects outside the
  journal remain reviewable but are not automatically undoable.
- Managed worktrees require an explicit base and dirty-source policy. Integration
  checks source HEAD, affected contents, and index entries, then journals file
  changes while preserving the index; it does not merge commit history. Cleanup
  rejects unintegrated content, staged changes, changed branches, ignored files,
  and private session storage.
- Verified linked worktrees can inspect shared control metadata. Metadata
  mutations require the original repository root. A separate Joe instance can
  run in a managed directory; concurrent worker assignment remains integration work.

Code: `src/utils/src/{git,changes}.rs`, `src/utils/src/git/worktrees.rs`,
`src/utils/src/workspace/unix/edits.rs`,
`src/tools/src/{git,apply_patch,review_changes,undo_changes,worktree}.rs`, and
`src/actors/src/change_control.rs`.

Limits: Git control paths must be ordinary files/directories. External object
alternates, configuration includes, bare repositories, submodules, and non-UTF-8
paths are unsupported. Git inventory uses `.gitignore` independently of
discovery-only `.ignore` rules.

Acceptance: Joe can review and explain its complete change while preserving
existing and concurrent user edits. Combined M6/M7/M8 checks cover worker journal
attribution, root review and undo, writer conflicts, scoped access, and
managed-process cleanup.

**Current bounds**

| Surface | Enforced bound |
| --- | --- |
| Cargo execution | 1–300 seconds, default 300; 16 MiB per output stream; no network, including localhost |
| Managed targets | At most eight starts per turn |
| File reads / search | 16 MiB per file; 32 MiB search output; at most 1000 context lines on either side |
| Discovery | 250,000 visited entries or 32 MiB of paths; pages default to 200 and cap at 1000 |
| Instructions | 64 KiB per source; 256 KiB active sources |
| Session artifacts | Outputs above 8 KiB archived; up to 64 MiB per artifact; retrieval pages up to 4096 bytes |
| Session storage | 1 GiB LMDB map; exhaustion stops continuation |
| Git/change review | 32 MiB Git text output; 64 MiB baseline content, serialized journals, or aggregate review |
| Workers | Four active; depth one; 32 starts and 2,000,000 allocated tokens per session |
| Worker budget | 1024–500,000 tokens; 1–300 seconds; 1–32 provider requests |

Hard limits fail explicitly; paginated discovery reports truncation. See the
[README](README.md) for current Cargo inputs, output fields, Git behavior, and
worktree operation details.

**M9 — Planning and user interaction — complete**

- `/plan` enters read-only planning and `/implement` explicitly restores
  implementation mode. Mode changes require an idle turn or finished cleanup.
  Shared dispatch policy denies all Cargo operations, patches, undo, worktree
  mutations, and dynamically selected write workers. Read workers inherit the
  mode; worktree listing remains available. Session-only interaction tools do
  not acquire filesystem leases.
- `update_plan` persists 1–16 steps with stable IDs, acyclic dependencies,
  acceptance criteria, pending/in-progress/completed/blocked states, and
  optimistic revisions. Starting requires completed dependencies; completion
  requires an in-progress predecessor and recorded successful evidence with
  explanations. Blocked steps require a reason. The runtime verifies evidence
  provenance; the model assesses whether it satisfies the acceptance criteria.
- Plans, mode, and bounded evidence catalogs persist through compaction,
  resume, and forks. Runtime planning state remains separate from operating
  instructions. New input and question answers mark an existing plan for
  reconciliation, blocking edits, Cargo and final completion until updated.
  Completed steps must be reopened after requirements change.
- `request_user_input` adds structured questions to the existing session store.
  `/questions` lists them; `/answer <id> choice <id>` and `/answer <id> text <text>`
  submit typed answers. Required questions stop new dispatch, settle the current
  batch, clean up owned work, and wait explicitly. Optional questions permit
  independent continuation. Answers cannot widen project permissions.
- Answers during tools commit immediately and enter history after the complete
  tool exchange. Question resolution and plan reconciliation markers commit
  atomically. Invalid choices, duplicate answers, reused IDs and implicit answers
  are rejected. Resume waits for input with pending questions restored; clear/new
  sessions reset interaction state and preserve the archive.
- Ordinary active-turn input queues FIFO. `/steer <correction>` cancels the active
  turn and its queued follow-ups, awaits cleanup, then continues the corrected
  task. Accepted results survive; uncertain operations are not replayed.
- The TUI shows mode, plan progress, pending questions, queue size, workers and
  the last validation outcome. Worker streams cannot overwrite the root stream.
  Vim editing and transcript behavior remain covered by deterministic tests.
- Question transitions and limits share one domain type across live state and
  session replay. Question gates, plan review, and UI progress use enums.
  Plan graph construction checks dependencies and evidence; step transitions
  check state changes. Saved question fields remain compatible with prior sessions.

Code: `src/common-models/src/interaction.rs`,
`src/common-models/src/interaction/{plan,question}.rs`,
`src/actors/src/{interaction_control,interaction_policy,session,turn_machine}.rs`,
`src/actors/src/tools/{request_user_input,update_plan}.rs`,
`src/commands/src/command.rs`, and `src/app/src/tui.rs`.

Limits: eight pending questions, six choices per question, 256 question IDs per
session, and a 256-entry evidence catalog retaining sources cited by the current
plan. Workers report questions to their parent; only the root changes the shared
plan or asks the user. Mode changes never grant filesystem or sandbox permissions.

Acceptance covers both root modes, denied mutations/processes, worker inheritance,
required/optional questions, answers during tools and cleanup, restart, compaction,
forks, steering, project boundaries, and clear/new sessions with waiting queues.

**M10 — Skills and MCP — planned**

- Discover global/repository skill metadata and load full instructions on demand.
  Support explicit selection, bounded implicit discovery, provenance, and
  references resolved through workspace policy.
- Treat skills as guidance. Scripts must map to supported typed operations or
  trusted configured integrations; skills cannot bypass the project boundary.
- Add user-configured MCP servers with namespaced tools, preserved JSON Schema,
  resources, progress, structured errors, bounded results, and timeout/cancellation.
  Implement and advertise a tested protocol subset.
- Support HTTP and stdio through explicit runtime network/process policy.
  Stdio programs/arguments come from trusted configuration. Treat annotations as
  advisory and independently enforce workspace and plan-mode restrictions.
  Disable integrations whose side effects cannot be confined.
- Keep credentials outside events, redact secrets, and bind configured access to
  server/account identity without an interactive grant flow. Implement
  authenticated-server/OAuth support as a separate slice with expiry/refresh tests.
- Keep startup lazy and integrations optional so core Rust tasks work offline.

Implementation areas: skill catalog, MCP client, provider/schema adapters,
runtime configuration/policy, and TUI discovery.

Acceptance tests: lazy loading, scoped references, conflicting guidance,
unsupported script execution, local fake HTTP/stdio servers, nested schemas,
duplicate names, startup failure, cancellation, malformed/oversized responses,
denied effects, credential refresh/redaction, and inherited plan-mode policy.

**Next work**

1. Implement M10 in separate skill, MCP transport/policy, and authentication slices.
2. Retain one writer per shared workspace. Require explicit worktree routing and
   ownership before allowing simultaneous writers; creation alone does not enable them.

**Recorded validation and rollout**

Latest recorded runs on 2026-09-08:

| Code state | macOS ARM64 | Linux ARM64 |
| --- | --- | --- |
| Main after M6/M8 integration | 249 workspace tests; workspace check; all-targets Clippy | 249 workspace tests; workspace check |
| M7 branch after M6 integration | 249 workspace tests; workspace check; all-targets Clippy | 249 workspace tests; workspace check |
| Combined M7/M8 after standards refactor | 271 workspace tests; workspace check; all-targets Clippy | 271 workspace tests; workspace check |
| M9 planning and user interaction | 290 workspace tests; workspace check; all-targets Clippy | 290 workspace tests; workspace check; all-targets Clippy |
| M9 after standards refactor | 291 workspace tests; workspace check; all-targets Clippy | Not rerun: Docker daemon did not respond |

Commands: `cargo test --workspace --offline`,
`cargo check --workspace --offline`, and
`cargo clippy --workspace --all-targets --offline`. The historical 249-test runs
cover different code states; the combined run exercises the integrated
implementation. Recorded checks pass
with existing warnings; changed Rust files pass formatting and whitespace
checks. Workspace-wide formatting has pre-existing differences.

Linux runs use Rust 1.95 Bookworm with rustfmt, Clippy, Bubblewrap, and
nested namespaces enabled.
Unsupported nested sandbox fixtures may skip; outer isolation tests still run.
Live provider checks, native Windows, and model/task-performance comparisons
remain unverified.

- Include focused regression coverage and relevant documentation/configuration
  updates with each implementation slice.
- Run affected crate tests and workspace compilation checks; run the full suite
  at integration milestones. Avoid unrelated mass formatting.
- Exercise both modes for shared runtime changes and macOS/Linux sandbox behavior
  for execution changes. Keep provider adapter tests deterministic and offline.
- Compare fixed Rust tasks with the same provider, model, reasoning effort, inputs,
  and resource budget. Record solved tasks, unintended edits, validation results,
  tokens, latency, and interruptions; repeat trials to expose variability.
- Mark milestones complete only after acceptance and dependent integration checks
  pass. Document limits explicitly.

Design references: [reasoning state](https://developers.openai.com/api/docs/guides/reasoning#keeping-reasoning-items-in-context),
[compaction](https://developers.openai.com/api/docs/guides/deployment-checklist#leverage-compaction),
[OS sandbox comparison](https://learn.chatgpt.com/docs/agent-approvals-security#os-level-sandbox).
