# Agent Joe implementation plan

Status: the first M1 implementation slice and M2 are implemented and validated on macOS. M1 capability/transition work remains in progress; M3 and later milestones remain pending.

Cover all ten gaps identified in the Codex comparison while keeping Joe a Rust-focused agent with typed tools and no model-controlled shell. Delivery order follows dependencies, so context compaction follows the response format and turn lifecycle work it needs.

**Scope and operating decisions**

- Preserve the existing TUI, Vim bindings, provider choices, and `--simple` option. Both agent modes use the same runtime services.
- Keep process execution behind validated tool schemas. Add Cargo, Git, and integration capabilities through explicit operations, with no arbitrary command-string or shell tool.
- Make workspace reads and ordinary edits automatic within the configured policy. Request additional permissions only for an operation that actually needs them; remember narrowly scoped grants.
- Implement and validate process isolation on macOS first, then Linux. Other platforms must report unsupported isolation explicitly until an equivalent backend is validated.
- Keep existing provider configuration readable. Add runtime, workspace, and session settings separately from credentials, with backward-compatible defaults.
- Use recorded fixtures and local fake providers for deterministic tests. Live provider checks and model comparisons are a separate, explicitly configured validation path.
- Measure task correctness, preservation of existing changes, completion time, and token use. A new feature alone does not establish better coding performance.

**Coverage and delivery order**

| Milestone | Result | Comparison gaps covered | Depends on |
| --- | --- | --- | --- |
| M0 | Trustworthy build/test baseline and small task fixtures | Validation across all gaps | — |
| M1 | Correct instructions, provider state, and tool-call history | 1: model state | M0 |
| M2 | Cancellable turns, tool scheduling, and recoverable errors | 3: runtime reliability | M1 |
| M3 | Workspace policy and isolated process execution | 4: sandbox | M2 |
| M4 | Durable sessions, bounded context, and compaction | 2: context and sessions | M1–M3 |
| M5 | Full repository discovery and scoped instructions | 5: instructions; 7: discovery | M3–M4 |
| M6 | Complete typed Cargo validation and process results | 6: validation | M2–M5 |
| M7 | Direct work plus optional, bounded delegation | 8: worker coordination | M4–M6 |
| M8 | Git awareness, aggregate review, and change isolation | 9: Git | M3–M7 |
| M9 | Plan mode, tracked steps, questions, and steering | 10: collaboration | M4–M8 |
| M10 | Skills and controlled MCP integrations | 10: extensibility | M3–M5, M9 |

**Shared architecture**

Keep the existing crates initially and introduce focused modules as they become necessary:

- `clients`: provider capabilities, request construction, native response items, stream decoding, and provider-specific compaction.
- `actors`: turn lifecycle, session coordination, scheduling, worker registry, and pending questions/permissions.
- `tools`: validated operation schemas and declared effects; concrete tool implementations use shared execution services.
- `utils`: bounded filesystem/process operations, workspace policy, and low-level storage helpers. Keep dependencies acyclic; move shared contracts into a small leaf crate only if the existing dependency graph requires it.
- `analysis`: optional Rust semantic context layered over a complete workspace file inventory.
- `common-models`, `commands`, and `app`: typed progress/results and TUI flows for the same runtime operations.

The model receives a bounded view of session state. The persisted session contains the event history and references to full tool-output artifacts. Provider-native reasoning/compaction items remain separate from display text and are replayed only to a compatible provider.

**M0 — Baseline and test seams**

Current evidence:

- `cargo check --workspace --offline` passes with existing warnings after refreshing the commands crate's build fingerprint. An initial stale artifact referred to `Command::Plan`, which is absent from the current source. No source edit was needed.
- `cargo test -p clients -p actors -p tools --lib --offline` reaches the actor suite and fails both existing replay tests. One fixture uses `end_turn` while the current enum expects `EndTurn`; the stored stream fixture also does not decode into valid current events. These are pre-existing fixture/schema mismatches, and the combined command stops before the other suites run.
- `cargo test -p clients -p tools --lib --offline` passes: 1 client test and 6 tool tests. The actor fixture failures remain the known test baseline to address in M0/M1.
- The existing stream replay tests deserialize events but do not verify a complete request → tools → continuation → completion cycle.

Work:

- Record existing failures separately from new regressions. Address failures that prevent validation of these milestones without expanding into unrelated cleanup.
- Introduce a fake streaming provider and injectable tool executor as the M1/M2 changes need them.
- Add a small set of temporary Rust workspaces covering a bug fix, a multi-file change, feature-specific tests, a manifest/CI change, and a pre-existing dirty working tree.
- Keep all deterministic fixtures independent of personal configuration, credentials, network access, and the live repository's working directory.

Done when the relevant baseline results are recorded and new runtime behavior can be exercised without a paid model call.

**M1 — Model state and instruction correctness**

First slice implemented: operating instructions and delegated tasks use separate
request channels; clear refreshes workspace context; typed JSON tool arguments and
grouped results survive history reconstruction; complete OpenAI reasoning items
and message phases are replayed; Claude signatures are preserved; incompatible
cross-provider reasoning is rejected. Complete-batch validation prevents execution
of incomplete or malformed calls and failed/truncated provider responses.

Validation: `cargo check --workspace --offline` passes; `cargo test --workspace
--offline` passes all 73 tests, including eight actor regression tests and three
new provider request/configuration tests. The full suite ran outside the execution
sandbox because macOS system-proxy discovery panics inside it when the fixtures
construct HTTP clients. Tests make no live provider calls. Changed Rust files were
formatted and the diff checked for whitespace errors.

The public OpenAI route requests encrypted reasoning automatically. Other routes
can opt in with `request_encrypted_reasoning`; returned state is preserved without
assuming that every compatible endpoint accepts the optional request field.

The new deterministic replay tests cover the provider mapping, stream processing,
fake tool execution, history, and continuation request together. The historical
parse-only tests were replaced by these regression tests and a synthetic JSONL
fixture. No live provider request is required.

Remaining M1 work: general capability discovery/overrides for hosted search and
compaction, and model/provider transition handling beyond rejecting incompatible
reasoning. M0's broader task benchmark fixtures remain pending. M2 now supplies the
injectable transport and turn recovery described below.

Primary files: `src/clients/src/{llm,openai,openai_mappings,claude_mappings}.rs`, `src/actors/src/{actor,actor_state,batch,stream_processor}.rs`, and context/prompt assembly.

- Separate worker operating instructions from workspace context and user messages. Populate OpenAI instructions and Claude system content through the provider adapter. Clearing conversation history must retain the active operating configuration and regenerate appropriate workspace context.
- Preserve OpenAI reasoning items, identifiers, encrypted content, and required ordering through streaming, history, and the next request. Preserve Claude thinking signatures independently. Display summaries must not substitute for provider continuation data.
- Preserve native tool arguments as JSON values or original argument strings, including booleans, numbers, arrays, and nested objects. The current string-map conversion must not alter replayed calls or fail while recording a tool error.
- Record all calls from a response and their corresponding outputs with stable IDs. Execute only complete, validated calls; never execute a partial streamed argument buffer.
- Introduce provider capability checks for reasoning replay, compaction, hosted search, and supported request fields. Public OpenAI API behavior is not sufficient evidence that the Codex-auth endpoint, OpenRouter, or a local server implements every field.
- Keep provider-native state tied to its provider/session. Define compatible model switching and an explicit transition for incompatible providers without sending opaque state to the wrong endpoint.

Validation: deterministic round trips across multiple tool calls; an empty reasoning summary with encrypted content; nested JSON arguments; failed tool results; clear/new-session behavior; Claude signature preservation; unsupported capability fallback; request snapshots for each supported provider route.

Done when repeated tool turns preserve instruction roles, original call data, and all required continuation state.

**M2 — Turn lifecycle, cancellation, and scheduling**

Implemented and validated on macOS (2026-09-05):

- Both simple and delegated modes use the same turn runtime. Provider requests,
  streaming, tools, worker startup, and cleanup run in owned background tasks.
  Turn/operation IDs tag events, and obsolete events cannot mutate history.
- One `Turn<P>` carries shared turn data through provider execution, tools, and
  cleanup, with distinct `TurnId`, `OperationId`, and `WorkspaceRevision` types.
  Actor messages dispatch to phase-specific transitions. One `ToolBatch`
  owns accepted calls and their queued/running/completed outcomes; replay tests
  use this same representation. Duplicate and mismatched completions are ignored.
  Tool results share their invocation data and use a standard `Result` for the
  outcome. Definitions, execution, and context updates share one tool collection.
  Phase handlers consume typed states through expression-based control flow.
  Error categories are retained where recovery depends on them; other failures
  use ordinary errors. The M2 code has no explicit returns, `ensure!`, or comments.
- The runtime registers active providers, tools, workers, and processes. Interrupt,
  clear, and application shutdown cancel the task tree and await cleanup. Worker
  replies register before work starts; immediate completion, startup failure, and
  provider failure resolve the parent. Successful replies take precedence over
  the child's subsequent shutdown signal.
- New messages during work become visible FIFO follow-ups. Interrupt and clear
  cancel queued follow-ups. No provider continuation starts until the preceding
  provider task exits. Active steering and question/permission interfaces remain
  assigned to M9/M3; their waiting lifecycle states are defined here.
- Trusted tool effects allow four concurrent reads, share a workspace lease across
  workers, and serialize mutations and validation. The scheduler consumes read
  groups and exclusive operations through one tool cleanup path. Mutation attempts advance an
  in-memory workspace revision. Validation holds the workspace lease for its run;
  detecting changes from external editors remains M5/M8 work.
- Accepted tool calls enter history with individual pending outcomes. Completed
  results survive cancellation; interrupted operations retain uncertainty and
  unstarted calls are explicitly marked unexecuted. A failed or timed-out write
  stops automatic continuation and remaining writes. Repeated identical tool
  failures at the same revision stop after three attempts. Cargo validation
  failures retain diagnostics and are recorded as error results. A timed-out read
  also stops the batch before a subsequent write can start.
- Authentication, rate-limit, transport, truncation, context-overflow, invalid-input,
  tool, and worker failures have distinct handling. Transient provider failures
  retry at most twice after existing HTTP retries, using accepted history without
  rerunning tools. Complete non-tool content is retained on final interruption;
  partial argument buffers and calls from failed responses never execute.
- Provider recovery uses HTTP status and structured error codes. Tool and worker
  failures carry explicit categories and effect certainty; diagnostic wording
  cannot select recovery behavior. Workspace leases own their locks and update
  mutation revisions on release, after outstanding work has finished.
- Both adapters share byte-oriented SSE decoding with split UTF-8, LF/CRLF/CR
  framing, multiline data, keepalives, terminal errors, and premature-EOF checks.
  SSE events are limited to 16 MiB. Fake providers exercise whole turns without
  HTTP clients, credentials, personal configuration, or paid requests.

Validation: `cargo test --workspace --offline` passes all 110 tests and
`cargo check --workspace --offline` passes with existing warnings. Tests cover
request and tool interruption, stale stream/tool completions, queueing, bounded
concurrent reads, ordered writes, revision-bound validation, timeouts, repeated
failures, worker completion/failure/cancellation, and clear during tools. Panics
in preparation, execution, and output conversion preserve other read results and
prevent subsequent writes. Native
macOS tests cancel a running Rust test process and its descendant, verify process
reaping and empty resource registries, and drain both pipes beyond pipe capacity.
The final suite runs inside the execution sandbox: replay tests no longer construct
HTTP clients, avoiding the earlier macOS system-proxy fixture panic. Changed Rust
files are formatted and the diff passes the whitespace check.

Process cleanup uses Unix process groups and reaps direct children. Native Linux
execution has not been validated in this environment; non-Unix execution reports
an unsupported capability. Deliberate process-group escapes require M3 isolation.
Filesystem mutations that have already entered a blocking OS call are awaited
before cancellation releases the workspace lease; cancellation cannot roll them
back. The former Cargo test against the live working directory was replaced by
deterministic managed-process fixtures. Output artifacts, durable revisions and
journaling remain M4/M6 work.

Primary files: `src/actors/src/{actor,actor_state,supervisor,stream_processor}.rs`, worker tool adapters, and `src/common-models/src/tui_models.rs`.

- Give every turn and operation a stable ID and explicit lifecycle: ready, running, waiting for tools/input/permission, cancelling, completed, cancelled, or failed.
- Run provider requests and tools outside the actor message handler. Deliver progress and completion through tagged messages so the actor remains responsive.
- Maintain a registry of active model streams, tools, and workers. Cancel their task tree, stop/reap child processes, resolve pending replies, and discard late events from an obsolete turn.
- Add tool effect metadata. Allow bounded concurrent reads; serialize writes initially and coordinate validation against a stable workspace revision. File-level concurrency can follow after conflict detection exists.
- Make a new message during work an explicit steer or queued follow-up. Do not launch overlapping streams against the same mutable history.
- Distinguish authentication, rate-limit, transport, truncation, context-overflow, invalid-input, tool, and worker failures. Existing HTTP retries remain useful; add turn-level recovery for interrupted streams and worker failure.
- Handle incomplete output without losing already accepted state. Stop repeated identical failures with a useful recoverable outcome; never automatically replay uncertain side effects.
- Make SSE decoding handle UTF-8 split across chunks, event framing, keepalives, terminal errors, and premature EOF.

Validation: interrupt during a model request, test process, and delegated write; stale completion after cancellation; independent reads executing concurrently; writes executing in order; tool timeout; child failure resolving the parent; interrupted streams; steering without duplicate execution.

Done when cancellation completes promptly, no owned process/worker is left running, and every turn reaches a visible terminal or waiting state.

**M3 — Workspace policy and process isolation**

Primary files: `src/utils/src/{files,cargo}.rs`, `src/tools/src/{tool_defs,apply_patch,read_file,grep}.rs`, `src/analysis/src/rust_proj.rs`, and new policy/executor modules.

- Introduce a workspace handle with explicit readable/writable roots, protected paths, network policy, and scoped permission decisions. Carry it through every tool and worker.
- Resolve relative paths against the workspace handle instead of process-global CWD. Check existing paths and the nearest existing parents of creation targets. Cover symlinks, traversal, moves, deletes, and filesystem race conditions with descriptor-based access or equivalent OS enforcement.
- Protect agent credentials/session storage and repository control directories from ordinary model writes. Explicit grants must identify the operation and resource, and cannot be minted by a worker.
- Route all child processes through a typed executor with explicit executable/arguments, working directory, sanitized environment, bounded output, timeout, cancellation, and process-tree cleanup.
- Isolate Cargo tests, build scripts, proc macros, and analysis startup. Audit rust-analyzer workspace loading and proc-macro helpers; either launch them through an isolated analysis helper or disable executable analysis features until isolation is available.
- Implement a macOS backend and Linux backend with equivalent documented filesystem/network behavior. Probe availability at startup. Unsupported isolation must produce a capability error or require a clearly configured external isolation boundary, never silently fall back to unrestricted execution.
- Separate the networked model client from repository-code execution. Add narrow network grants for dependency fetching and configured integrations where needed.
- Use a single permission broker and TUI event flow. Ordinary allowed operations continue automatically; requests show the concrete resource/action and optional narrow remembered scope.

Validation: outside-root reads/writes, absolute paths, traversal, symlink swaps, protected files, inherited credential variables, test/build-script/proc-macro escapes, network denial, permitted dependency fetching, backend unavailability, and cancellation of descendants. Run OS-specific integration tests on their actual platforms.

Done when all executable repository code and filesystem tools respect the same policy, including before the first model turn.

**M4 — Sessions and context management**

Primary files: `src/actors/src/actor_state.rs`, client request models, and new session/history modules; commands and TUI session controls.

- Use versioned per-session JSONL events plus atomic snapshots and bounded output artifacts under Joe's existing storage directory. Make the location configurable for tests and protect it with appropriate permissions.
- Persist session/workspace identity, user messages, provider-native output, tool intent/results, worker linkage, usage, pending questions, and turn status. Keep credentials and authorization headers out of records.
- Journal side-effect intent before execution and record completion afterward. A crash between them leaves an uncertain operation to reconcile with workspace state; resume must never blindly repeat it.
- Add `/sessions`, `/resume`, `/fork`, `/compact`, and explicit new/clear semantics. Recover valid records from a truncated final journal entry and reject unsupported schema versions clearly.
- Fork conversation state separately from filesystem state. Revalidate workspace identity, current instructions, and effective permissions on resume; a saved session cannot increase current access.
- Bound individual reads/search/process outputs and retain full content as artifacts with a retrieval mechanism. Budget instructions, user constraints, recent complete tool exchanges, and the next response before optional context.
- Use provider-native compaction when the route supports it, preserving returned opaque items intact. Otherwise summarize only completed older exchanges and retain recent complete exchanges, current requirements, pending work, and failure/validation evidence.
- Trigger compaction before the context ceiling, with a user-visible indication and recovery path if compaction fails. Distinguish per-request context size from cumulative usage.

Validation: resume after restart, truncated journal, incompatible schema/provider, crash between write intent and result, fork history isolation, giant test output, forced low context limit, repeated compaction retaining user constraints, and no orphan tool calls/results.

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
- Return command details, exit code, duration, diagnostics, timeout/cancel status, and bounded stdout/stderr with full artifact references. Preserve Cargo startup/dependency errors currently lost by discarding stderr.
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
- Keep permission decisions distinct from clarification and planning. Required answers remain pending until answered; elapsed time is never consent.
- Finish the steering/queue UI from M2: show what will affect the active turn, what is queued, and what was cancelled. Reconcile changed requirements with the existing plan instead of starting overlapping work.
- Display worker/progress/validation status compactly while preserving existing Vim interaction and transcript behavior.

Validation: forbidden writes/processes in plan mode, worker policy inheritance, unanswered required question, optional answer arriving during tools, resume with pending question, corrected requirements during a turn, and clear/new-session behavior.

Done when a user can plan, clarify, steer, interrupt, and resume a task without losing intent or accidentally authorizing an action.

**M10 — Skills and MCP**

Primary files: new skill catalog and MCP client modules, provider/tool schema adapters, runtime policy/configuration, and TUI discovery controls.

- Discover global and repository skill metadata, load full instructions on demand, and resolve references through workspace policy. Support explicit selection and bounded implicit discovery with source attribution.
- Treat skill instructions as guidance. Scripts must map to supported typed operations or a separately configured integration; a skill cannot introduce a shell escape or bypass permissions.
- Add user-configured MCP servers with namespaced tool discovery, JSON Schema preservation, bounded results, timeout/cancellation, resources, progress, and structured errors. Start with a tested protocol subset and advertise it accurately.
- Support configured HTTP and stdio transports through the shared network/process policy. Stdio programs and arguments come from trusted configuration, never arbitrary model input.
- Treat tool annotations as advisory. Derive effective read/write/network/destructive permissions from trusted configuration and enforce plan-mode restrictions independently of server claims.
- Keep server credentials in configuration/credential storage, redact them from events, and scope remembered permissions to server identity and account. Include authenticated-server/OAuth support as a separate implementation slice with expiry and refresh tests.
- Keep startup lazy, schemas/results bounded, and integrations optional so basic Rust tasks work offline without any MCP server.

Validation: lazy skill loading and reference scope, conflicting skill guidance, a skill attempting unsupported execution, local fake MCP servers for both transports, nested schemas, duplicate tool names, failed startup, cancellation, malformed/oversized responses, denied side effects, credential refresh/redaction, and plan-mode restrictions.

Done when skills and configured integrations work through the same session, lifecycle, and permission system as built-in tools.

**First implementation slice**

Start M1 with the smallest complete change: separate operating instructions from user/context messages; preserve OpenAI reasoning and original JSON tool arguments through one full tool cycle; preserve the existing Claude route; and add deterministic multi-turn request tests. Include clear/new-session behavior so the initial fix is not immediately lost when history is reset.

Follow with M2's nonblocking execution and cancellation before adding new tools. This gives every later milestone a consistent execution path and a testable failure model.

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
