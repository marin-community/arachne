# Track coordination implementation

This implements the initial priorities in [workflow-design.md](workflow-design.md), using Arachne's existing Loom session, channel, tag, resource, and integration primitives. The policy remains permissive and advisory; no workflow state machine or mandatory review gate is introduced.

- Coordinator, worker, and reviewer guidance lives in `skills/coordinator.md`, `skills/worker.md`, and `skills/reviewer.md`. It covers context protection, bounded/recursive delegation, risk-based review, durable handoffs, Track brief curation, parent-first escalation, and event-driven waiting. Guidance travels in both the launch-guidance fields and the durable opening turn because older Loom servers ignore the former. A `branches.update` metadata update restores the concise objective after launch while preserving `goal.txt`, including the role and launch-profile instructions. `sessions.update` must not be used for this cleanup: it replaces the startup prompt before a terminal agent may read it. Metadata failure does not retry an already completed launch.
- Dashboard delegations receive worker guidance. Recursive delegation and reparenting promote only root conversations to Tracks, preserving the accepted Track target independently of conversation ancestry.
- The existing Delegate and Send to thread actions remain the launch and provenance-preserving messaging paths. Agent guidance explains their CLI equivalents and typed result publication.
- Launch reviewer creates a fresh child Thread from the candidate branch with its objective, source checkout, integration target, and editable review brief. It receives no implementation transcript. Review findings return to the original worker; preparation still runs in the source Thread. Reviewer Threads are excluded from integration candidates and cannot be integrated as implementations. Tag-write failures are surfaced as warnings without falsely failing a successful reviewer launch.
- Handoffs exposes typed durable results separately from routine mailbox traffic, with source-thread drill-down. Reads preserve channel read markers and search each channel's latest 500 messages; the view shows at most 60 results across the 20 most recently active relevant channels. Full history remains available through the source Thread and Loom channels.
- The inspector Landing queue reads explicit `change:<id>` v1 declarations, not Thread branches/statuses/titles. Each selected output pins original identity, base/head/path scope and completion, with a separate `change_acceptance:<id>` receipt. Multiple outputs and later outputs from an accepted Thread remain independent. Legacy `integration_result` is historical evidence only. Review-only and unmarked review Threads with no outputs have no Land obligation. Historical completion does not create a new candidate.
- Selected review/Prepare/Land carry the same declared scope. Range previews exclude checkout edits; working scopes pin HEAD/version/paths. Stale selection or conflicting duplicate/receipt metadata fails closed. Manual Land queues noninterrupting selected wrap-up and complete coordinator review/repair/commit/validation/acceptance, default squash into the live initialized canonical ref. Existing explicit legacy commands remain compatible. Queued requests never claim success.
- Autoland consumes only explicit outputs in authenticated intended final own-channel result delivery, rereads live policy/identity/scope before writes, and preserves interruption/no-auto-resume protections. Track Publish remains separate. No scheduler or agent polling is added.
- Needs You and OS notifications require explicit attention/blocked tags or live tool approvals. Errors, completion, review activity, waiting, and parent-help requests remain visible facts without automatically notifying a human.
- Thread rows and home views display advisory working, reviewing, ready, stale/conflicted, needs-parent, waiting, integrated, and resting labels. Inspector tabs scroll to keep all views accessible in a narrow rail.

The specification explicitly places automation triage Threads, lightweight model routing, event coalescing, sleeping-worker wakes, maintained mergeability checks, and richer durable Track state after this initial phase and dogfooding. These are not added as a new scheduler or control-plane service here. This phase displays reported readiness facts; it does not continuously verify candidate mergeability. Durable Track brief maintenance is agent guidance using existing versioned artifacts.

Baseline validation before explicit Change v1 (historical counts):

- `npm test`: 141 passed.
- `npm run typecheck`: passed.
- `npm run build`: passed.
- `cargo test --manifest-path src-tauri/Cargo.toml --locked`: 100 passed (91 unit, 4 decode, 5 integration-target).
- `git diff --check`: passed.

Regression coverage includes independent reviewer context/source routing, legacy-server role delivery after metadata cleanup and a delayed startup-prompt read, avoiding duplicate launches after metadata failure, recursive Track ownership, stale readiness precedence, explicit human escalation, and handoff provenance. No live agent launch was performed as validation. Running Arachne instances must be reopened after rebuilding the backend to receive new launch behavior; existing sessions do not receive retroactive role guidance.

The Change v1 implementation is intentionally a durable metadata/result contract,
not a new transactional Git service. Agents still perform review, repair,
validation and acceptance under the full guarded prompt. Remote arbitrary-range
previews are unavailable in existing Loom APIs. Existing sessions receive new
role guidance only on future launches; migration requires explicit declaration
of original output scopes and verified per-output accepted receipts, never
automatic historical replay.
