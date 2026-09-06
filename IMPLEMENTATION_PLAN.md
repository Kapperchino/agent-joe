# Agent Joe implementation plan

Status: M1–M3 are complete. M4 — sessions and context management — is in progress; M5–M10 remain planned.

Cover all ten gaps identified in the Codex comparison while keeping Joe a Rust-focused agent with typed tools and no model-controlled shell. Delivery order follows dependencies, so context compaction follows the response format and turn lifecycle work it needs.

**Scope and operating decisions**

- Preserve the existing TUI, Vim bindings, provider choices, and `--simple` option. Both agent modes use the same runtime services.
- Keep process execution behind validated tool schemas. Cargo is the only current executable operation. The shared sandbox accepts registered operation types so future tools can reuse it without adding arbitrary command-string or shell access.
- Keep every agent operation inside a fixed project boundary. Allowed operations run automatically; denied operations fail without permission prompts or access escalation.
- Isolate executable repository operations on macOS and Linux. File tools and passive analysis use the project filesystem policy in process. Unsupported or unavailable isolation disables executable operations without disabling file tools.
- Keep existing provider configuration readable. Add runtime, workspace, and session settings separately from credentials, with backward-compatible defaults.
- Use recorded fixtures and local fake providers for deterministic tests. Live provider checks and model comparisons are a separate, explicitly configured validation path.
- Measure task correctness, preservation of existing changes, completion time, and token use. A new feature alone does not establish better coding performance.

**Coverage and delivery order**

| Milestone | Status | Result | Comparison gaps covered | Depends on |
| --- | --- | --- | --- | --- |
| M0 | Baseline established | Trustworthy build/test baseline and small task fixtures | Validation across all gaps | — |
| M1 | Complete | Correct instructions, provider state, and tool-call history | 1: model state | M0 |
| M2 | Complete | Cancellable turns, tool scheduling, and recoverable errors | 3: runtime reliability | M1 |
| M3 | Complete | Workspace policy and reusable isolation for Cargo | 4: sandbox | M2 |
| M4 | In progress | Durable sessions, bounded context, and compaction | 2: context and sessions | M1–M3 |
| M5 | Planned | Full repository discovery and scoped instructions | 5: instructions; 7: discovery | M3–M4 |
| M6 | Planned | Complete typed Cargo validation and process results | 6: validation | M2–M5 |
| M7 | Planned | Direct work plus optional, bounded delegation | 8: worker coordination | M4–M6 |
| M8 | Planned | Git awareness, aggregate review, and change isolation | 9: Git | M3–M7 |
| M9 | Planned | Plan mode, tracked steps, questions, and steering | 10: collaboration | M4–M8 |
| M10 | Planned | Skills and controlled MCP integrations | 10: extensibility | M3–M5, M9 |

**Shared architecture**

Keep the existing crates initially and introduce focused modules as they become necessary:

- `clients`: provider capabilities, request construction, native response items, stream decoding, and provider-specific compaction.
- `actors`: turn lifecycle, session coordination, scheduling, worker registry, and pending questions.
- `tools`: validated operation schemas and declared effects; concrete tool implementations use shared execution services.
- `utils`: bounded filesystem operations, workspace policy, a shared sandbox for registered executable operations, and low-level storage helpers. Keep dependencies acyclic; move shared contracts into a small leaf crate only if the existing dependency graph requires it.
- `analysis`: optional Rust semantic context layered over a complete workspace file inventory.
- `common-models`, `commands`, and `app`: typed progress/results and TUI flows for the same runtime operations.

The model receives a bounded view of session state. The persisted session contains the event history and references to full tool-output artifacts. Provider-native reasoning/compaction items remain separate from display text and are replayed only to a compatible provider.

**Completed foundation**

The build/test baseline and deterministic provider/tool test seams are established.
Tests use temporary workspaces and fake providers without personal configuration,
credentials, live model calls, or changes to the working repository. Broader task
benchmarks belong to validation and rollout below.

**M1 — Model state and instruction correctness — complete**

- Operating instructions, workspace context, and user/delegated messages use
  separate request channels. Clearing history preserves operating configuration
  and refreshes workspace context.
- Provider-native reasoning, identifiers, encrypted content, message phases, and
  Claude signatures survive history and continuation. Incompatible reasoning is
  rejected instead of being sent to another provider. Optional request fields
  remain route-specific; public OpenAI requests encrypted reasoning automatically,
  while other routes can opt in with `request_encrypted_reasoning`.
- Native JSON arguments, stable call IDs, and grouped tool results survive replay.
  Only complete, validated tool batches execute; malformed, partial, failed, or
  truncated responses cannot trigger tool side effects.
- Deterministic round trips cover provider mapping, streaming, tool execution,
  history, continuation, and clear behavior.

Primary files: `src/clients/src/{llm,openai,openai_mappings,claude_mappings}.rs`,
`src/actors/src/{actor,actor_state,batch,stream_processor}.rs`, and context/prompt assembly.

**M2 — Turn lifecycle, cancellation, and scheduling — complete**

- Simple and delegated modes share a typed turn state machine, stable turn and
  operation IDs, owned background tasks, and a resource registry. Stale or duplicate
  events cannot alter active history; worker completion and failures resolve parents.
- Interrupt, clear, and shutdown cancel owned tasks and await cleanup. Process
  groups are signalled and leaders reaped. Blocking filesystem work finishes before
  its workspace lease is released; cancellation does not roll back completed writes.
- New messages queue as FIFO follow-ups. Reads have bounded concurrency; writes
  and validation share an exclusive workspace lease and revision tracking.
- Accepted results survive interruption. Unstarted calls are marked unexecuted,
  uncertain side effects are not replayed, and failures stop unsafe continuation.
  Provider recovery uses structured categories and bounded retries.
- Shared SSE decoding handles split UTF-8, framing, keepalives, terminal failures,
  and premature EOF with bounded event sizes. Fake-provider tests exercise whole
  turns, concurrency, cancellation, recovery, and worker lifecycle races.

Primary files: `src/actors/src/{actor,actor_state,supervisor,stream_processor}.rs`,
worker tool adapters, and `src/common-models/src/tui_models.rs`.

**M3 — Workspace policy and reusable sandbox — complete**

- `WorkspacePolicy` and descriptor-based filesystem helpers enforce the fixed
  project boundary for tools, workers, previews, watchers, and logs. Validation
  belongs to path/file types. Traversal, symlinks, hard links, special files, and
  read-only directory aliases cannot redirect ordinary file operations. Writes
  replace sibling files atomically and preserve ordinary permissions. Joe storage
  is inaccessible to tools; repository control directories deny writes.
- File tools and source analysis run in process. Analysis is passive, with an
  in-memory symbol cache; startup executes no Cargo, build scripts, proc macros,
  or sandbox probes. Dependency resolution and macro expansion are unavailable.
- The shared `Sandbox::output` accepts registered `SandboxOperation` types through
  a crate-controlled sealed trait. `CargoOperation` is its only production caller,
  with validated `Check` and `Test` operations. Future executable tools register
  typed operations and reuse isolation, limits, and cancellation without exposing
  arbitrary commands or shell text.
- macOS uses Seatbelt; Linux uses Bubblewrap namespaces, dropped capabilities,
  and a sealed seccomp filter. Cargo runs offline with a clean environment,
  project-local writes, read-only toolchains/caches, and no network or unrelated
  host signals. Unsupported or unavailable isolation fails closed while file tools
  remain available. Linux requires `/usr/bin/bwrap` and host namespace support.
- Process workspace construction permits existing hard links only when every
  alias is inside the workspace with the same access permissions. This supports
  Cargo's linked build artifacts while rejecting outside and protected aliases;
  sandboxed processes still cannot create new hard links.
- Each command owns a fresh temporary directory that is removed when execution
  finishes or is cancelled. Test workspaces there can create private session
  storage; existing project storage remains inaccessible, including through
  aliases. macOS permits System V semaphores and read-only process information
  for LMDB and session ownership. Semaphore access is not scoped by workspace
  on macOS; normal operating-system ownership permissions still apply.
- Execution is limited to five minutes and 16 MiB per output stream. Descendants
  remain confined after changing sessions; guaranteed termination of deliberately
  detached descendants is outside the accepted scope. There is no permission
  broker, access escalation, unconfined fallback, or custom process scanning.

Primary files: `src/utils/src/{workspace,files,cargo,sandbox}.rs`,
`src/utils/src/{workspace,sandbox}/*`, analysis startup/cache, and watcher actors.

Boundary limits: trusted provider connections and user setup stay outside the model
tool surface. Concurrent hostile host processes are outside the threat model, and
multi-operation patches remain nontransactional.

Latest validation (2026-09-06): `cargo test --workspace --offline` passes 146 tests
on macOS and 147 on Linux, both ARM64. `cargo check --workspace --offline` passes
on both platforms with existing warnings. Coverage includes filesystem escapes,
passive startup, Cargo build scripts/proc macros/tests, actor cancellation, inherited
credentials, network denial, output limits, and timeouts. Changed Rust files pass
formatting and whitespace checks; existing worker import ordering remains the
workspace formatting baseline.

**M4 — Sessions and context management**

Primary files: `src/actors/src/actor_state.rs`, client request models, and new session/history modules; commands and TUI session controls.

- Use the existing LMDB technology (`heed`) for versioned per-session events, atomic snapshots, and bounded output artifacts under protected project-local Joe storage. Commit events and snapshots together in durable transactions. Make the location configurable for tests and protect it with appropriate permissions.
- Persist session/workspace identity, user messages, provider-native output, tool intent/results, worker linkage, usage, pending questions, and turn status. Keep credentials and authorization headers out of records.
- Journal side-effect intent before execution and record completion afterward. A crash between them leaves an uncertain operation to reconcile with workspace state; resume must never blindly repeat it.
- Add `/sessions`, `/resume`, `/fork`, `/compact`, and explicit new/clear semantics. Recover the last committed LMDB transaction after a crash and reject unsupported schema versions clearly.
- Fork conversation state separately from filesystem state. Revalidate workspace identity, current instructions, and fixed project policy on resume; a saved session cannot increase current access.
- Bound individual reads/search/process outputs and retain full content as artifacts with a retrieval mechanism. Budget instructions, user constraints, recent complete tool exchanges, and the next response before optional context.
- Use provider-native compaction when the route supports it, preserving returned opaque items intact. Otherwise summarize only completed older exchanges and retain recent complete exchanges, current requirements, pending work, and failure/validation evidence.
- Trigger compaction before the context ceiling, with a user-visible indication and recovery path if compaction fails. Distinguish per-request context size from cumulative usage.

Validation: resume after restart, abandoned transactions, incompatible schema/provider, crash between write intent and result, fork history isolation, giant test output, forced low context limit, repeated compaction retaining user constraints, and no orphan tool calls/results.

First slice: durable sessions

- Shared simple/worker runtime persists conversation history, native provider items,
  queued user messages, tool batches, intent/results, usage, turn status, and worker
  parent IDs. Each event and updated snapshot commit in one LMDB transaction;
  provider configuration and authorization headers are not serialized.
- Storage uses `.turbo-code/sessions` with private directory/file permissions,
  descriptor-based preflight checks, and transactional LMDB ownership per loaded
  session. Ownership records contain a process identity and unique token;
  dead owners can be reclaimed, and stale handles cannot access or release a
  newer owner's session. Process identity includes boot and start time to
  distinguish reused PIDs.
  `Runtime::with_session_namespace` configures a separate protected namespace for
  tests. Saved data cannot supply workspace roots or grant access.
- `/sessions`, `/resume`, `/resume <id>`, and `/new` are available. Bare `/resume`
  opens a searchable picker with recent conversations first, keyboard selection,
  cancellation, and transcript restoration. Only nonempty root sessions for the
  current provider appear; LMDB ownership is checked when a session is selected.
  `/clear` cancels active work
  and starts a new session while preserving the previous session. Resume requires
  an idle actor, revalidates workspace identity and provider route, refreshes current
  workspace context/instructions, and waits for a user message before continuing.
- Tool intent commits before execution and completion commits before reporting.
  Recovery supplies matched results for every saved call: completed, unexecuted,
  or uncertain. It never launches saved operations. Storage failures stop automatic
  continuation; shutdown retains results committed before actor delivery.
- The environment currently has a 1 GiB map limit. History and outputs remain
  inline and unbounded within that limit; map exhaustion fails closed. Artifacts,
  history forks, context budgeting, compaction, and pending question state remain
  subsequent slices. No older session format exists to migrate.

Slice validation (2026-09-06): `cargo test --workspace --offline` passes 178 tests
on macOS ARM64, including process-exit recovery, abandoned transactions, map
exhaustion, exclusive ownership across processes, schema/provider/workspace checks,
durable simple/worker turns, picker navigation and cancellation, narrow terminal
rendering, transcript restoration, and confined Cargo hard links. `cargo check --workspace --offline` passes with
existing warnings. Changed Rust files pass formatting and whitespace checks.
The 14 session tests also pass through Joe's own sandbox, including LMDB
initialization, competing processes, and crash recovery. Sandbox regressions
cover temporary storage, semaphore and process identity access, cleanup, and
denial of aliases to saved project storage.
Linux and Windows have not been validated for this slice.

Done when a long task can survive compaction and restart without losing requirements or duplicating side effects.

**M5 — Repository discovery and instructions**

Primary files: `src/analysis/src/contexts/*`, `src/actors/src/background_actors/*`, `src/tools/src/{grep,read_file}.rs`, and new workspace inventory/instruction modules.

- Build a deterministic, ignore-aware file inventory independent of Rust symbol discovery. Include discoverable manifests, documentation, CI, fixtures, and files with no Rust symbols; permit explicit reads of ignored paths when policy allows.
- Add filename/path search, include/exclude filters, literal/regex modes, result limits, and clear truncation metadata. Keep directory listing available through a bounded tool surface.
- Use consistent one-based line numbers with documented exclusive range ends. Return structured range errors instead of panics; support newly created and non-Rust files without a cached semantic line index.
- Share watcher startup between simple and worker modes. Update create/modify/delete/rename events and invalidate analysis after successful edits; refresh or read from disk if watcher events lag.
- Load global, repository, and applicable nested `AGENTS.md` guidance with documented precedence and provenance. Discover nested rules before editing their scoped paths. Keep retrieved file text and external content distinct from operating instructions.
- Add an instruction/context inspector showing active sources, scope, and truncation. Avoid repeatedly injecting the complete symbol map and accumulated worker summaries.

Validation: finding text in `Cargo.toml`, CI YAML, Markdown, and fixtures; ignored paths; empty/new files; Unicode ranges; post-edit reads in both modes; nested rules; conflicting instructions; symlink-root scope; and bounded results on a large workspace.

Done when Joe can discover and inspect every relevant allowed file with fresh line information and the correct scoped instructions.

**M6 — Typed validation and runnable Rust targets**

Primary files: `src/utils/src/cargo.rs`, `src/tools/src/cargo_{check,test}.rs`, new Cargo operation tools, and worker prompts.

- Add schema-controlled workspace/package, feature, target, test, example, and binary selection. Validate combinations and treat identifiers as values rather than allowing argument injection.
- Add formatting/check-format, Clippy, and typed run operations for examples/binaries. Support explicitly allowed program arguments and environment values through the shared policy. Preserve the no-shell constraint.
- Return command details, exit code, duration, diagnostics, timeout/cancel status, and bounded stdout/stderr with full artifact references. Preserve existing Cargo startup/dependency errors and stderr in the richer result schema.
- Support long-running Rust targets through managed process IDs, incremental output, polling, and stop operations; keep them in the turn/session process registry.
- Let workers add focused regression coverage when behavior warrants it. Record requested checks, checks actually executed, failures, and limitations; passing compilation alone is not proof of behavioral correctness.
- Reuse a validation result only when the relevant workspace revision, command parameters, and environment match. Run targeted checks before broader checks.

Validation: multi-package workspace; feature-gated regression; Clippy/format failure; invalid argument-like selectors; manifest parse error; stderr-only failure; huge output; timeout; cancellable example server; and exact structured command/result reporting.

Done when Joe can reproduce, fix, and validate representative Rust tasks through typed operations alone.

**M7 — Worker coordination**

Primary files: `src/actors/src/workers/*`, `src/actors/src/tools/{gather_context,make_changes,validate_rust}.rs`, and a shared worker registry.

- Give the root agent direct discovery/edit/validation tools. Delegate bounded independent work when useful; retain `--simple` as an explicit no-delegation mode.
- Define worker requests with objective, constraints, allowed tools/paths, relevant context references, completion criteria, and budgets. Share the parent's effective policy without widening it.
- Return typed results containing status, findings, changed files, validation evidence, unresolved issues, and artifact references. Keep a concise model-facing explanation alongside structured data.
- Register a worker and its completion channel before starting it. Support list/status, follow-up, cancellation, cleanup, and failure propagation; bound worker count/depth and token/time consumption.
- Start with one workspace writer and concurrent read workers. Require ownership or isolated worktrees before enabling simultaneous writers.
- Share selected, relevant context instead of appending every worker report to every future worker. Preserve explicit parent/user constraints in all handoffs.

Validation: direct completion of a small change; bounded delegation of independent investigation; registration/completion race; child timeout/failure; cancellation propagation; worker cleanup; budget exhaustion; overlapping write requests; and policy inheritance.

Done when delegation is optional, observable, cancellable, and cannot silently drop validation or widen access.

**M8 — Git and complete change review**

Primary files: new typed Git tools/executor operations, `src/tools/src/apply_patch.rs`, session workspace metadata, and TUI diff views.

- Add typed status, diff, show, and log operations with validated paths/revisions, stable machine-readable output, and external diff/textconv/pager execution disabled.
- Capture the working tree/index baseline at task start. Track Joe's intended edits separately from pre-existing changes and detect user edits made during a task.
- Preflight every file operation in a patch before applying it, reject stale content, and journal enough information to identify partial application. Stage replacement content safely and report partial failure accurately; do not promise atomicity across multiple files.
- Add aggregate task diff and review that include untracked files, renames, deletions, and staged/unstaged differences. Give the agent a chance to inspect the complete result before claiming completion.
- Add optional managed worktree creation for isolated tasks/parallel writers, with explicit handling of a dirty source tree, base commits, conflicts, and cleanup. A history fork alone must not imply a filesystem fork.
- Permit undo only for recorded Joe-owned edits whose current content still matches the recorded result. Surface conflicts rather than overwriting user work.

Validation: dirty index/tree, same-file user edits, untracked files, filenames with whitespace, rename/delete, patch failure halfway through application, external Git helper configuration, conflicting worktree integration, and guarded undo.

Done when Joe can explain and review its complete change while preserving existing and concurrent user edits.

**M9 — Planning and user interaction**

Primary files: `src/commands/src/command.rs`, actor session state, worker prompts, `src/common-models/src/tui_models.rs`, and `src/app/src/tui.rs`/widgets.

- Add `/plan` and an explicit return to implementation mode. Planning applies a read-only tool policy inherited by workers and integrations; mutating calls are rejected at dispatch, not merely discouraged in the prompt.
- Store a compact plan with step IDs, dependencies, acceptance criteria, and pending/in-progress/completed/blocked status. Persist it across resume and compaction; validate status transitions against reported evidence.
- Add a structured question tool with question ID, choices/free text, required/optional status, and a typed answer event. Let independent work continue while optional questions are pending.
- Use questions for clarification and planning only. Required answers remain pending until answered; answers cannot widen the project boundary.
- Finish the steering/queue UI from M2: show what will affect the active turn, what is queued, and what was cancelled. Reconcile changed requirements with the existing plan instead of starting overlapping work.
- Display worker/progress/validation status compactly while preserving existing Vim interaction and transcript behavior.

Validation: forbidden writes/processes in plan mode, worker policy inheritance, unanswered required question, optional answer arriving during tools, resume with pending question, corrected requirements during a turn, and clear/new-session behavior.

Done when a user can plan, clarify, steer, interrupt, and resume a task without losing intent or accidentally authorizing an action.

**M10 — Skills and MCP**

Primary files: new skill catalog and MCP client modules, provider/tool schema adapters, runtime policy/configuration, and TUI discovery controls.

- Discover global and repository skill metadata, load full instructions on demand, and resolve references through workspace policy. Support explicit selection and bounded implicit discovery with source attribution.
- Treat skill instructions as guidance. Scripts must map to supported typed operations or a separately configured integration; a skill cannot introduce a shell escape or bypass the project boundary.
- Add user-configured MCP servers with namespaced tool discovery, JSON Schema preservation, bounded results, timeout/cancellation, resources, progress, and structured errors. Start with a tested protocol subset and advertise it accurately.
- Support configured HTTP and stdio transports through the shared network/process policy. Stdio programs and arguments come from trusted configuration, never arbitrary model input.
- Treat tool annotations as advisory. Restrict configured integrations to the fixed project boundary and enforce plan-mode restrictions independently of server claims. Disable integrations whose side effects cannot be confined.
- Keep server credentials in configuration/credential storage, redact them from events, and bind configured access to server identity and account without an interactive grant flow. Include authenticated-server/OAuth support as a separate implementation slice with expiry and refresh tests.
- Keep startup lazy, schemas/results bounded, and integrations optional so basic Rust tasks work offline without any MCP server.

Validation: lazy skill loading and reference scope, conflicting skill guidance, a skill attempting unsupported execution, local fake MCP servers for both transports, nested schemas, duplicate tool names, failed startup, cancellation, malformed/oversized responses, denied side effects, credential refresh/redaction, and plan-mode restrictions.

Done when skills and configured integrations work through the same session, lifecycle, and project policy as built-in tools.

**Next implementation slice — M4**

Build bounded output artifacts and retrieval on the LMDB session foundation, then
budget request context and add `/compact` with provider-compatible fallback
summaries. Add conversation-only `/fork` without implying filesystem isolation.
Preserve current requirements, recent complete tool exchanges, and failure evidence
through repeated compaction; distinguish request context size from cumulative usage.

**Validation and rollout**

- Each implementation slice includes the behavior change, focused regression tests, and relevant documentation/configuration migration.
- Run the affected crate tests and `cargo check --workspace`; use `cargo test --workspace` at integration milestones. Establish formatting and Clippy baselines before making them mandatory, and avoid unrelated mass reformatting.
- Test both `--simple` and worker mode for every shared runtime change. Test provider adapters offline; keep live provider smoke tests opt-in and record unavailable routes as unverified.
- Run platform sandbox tests on macOS and Linux. Report native Windows coverage separately rather than inferring it from successful compilation.
- Compare a fixed set of Rust tasks before/after significant milestones using the same provider, model, reasoning effort, task inputs, and resource budget. Record solved tasks, unintended edits, check results, tokens, latency, and interruptions; repeat model-driven trials to expose variability.
- Mark a milestone complete only when its acceptance criteria and dependent integration checks pass. Document a capability limit instead of silently falling back to weaker behavior.

**Reference constraints**

- OpenAI recommends preserving reasoning items and tool exchanges between consecutive function calls: [reasoning state](https://developers.openai.com/api/docs/guides/reasoning#keeping-reasoning-items-in-context).
- Provider-native compacted output must be forwarded intact; support must be checked per route: [compaction guidance](https://developers.openai.com/api/docs/guides/deployment-checklist#leverage-compaction).
- Codex's local sandbox uses platform-specific OS enforcement, providing a comparison point for Joe's backend design: [OS-level sandbox](https://learn.chatgpt.com/docs/agent-approvals-security#os-level-sandbox).
