# Track coordination implementation

This implements the initial priorities in [workflow-design.md](workflow-design.md), using Arachne's existing Loom session, channel, tag, resource, and integration primitives. The policy remains permissive and advisory; no workflow state machine or mandatory review gate is introduced.

- Coordinator, worker, and reviewer guidance lives in `skills/coordinator.md`, `skills/worker.md`, and `skills/reviewer.md`. It covers context protection, bounded/recursive delegation, risk-based review, durable handoffs, Track brief curation, parent-first escalation, and event-driven waiting. Guidance travels in both the launch-guidance fields and the durable opening turn because older Loom servers ignore the former. A metadata update restores the concise objective after launch; failure does not retry an already completed launch.
- Dashboard delegations receive worker guidance. Recursive delegation and reparenting promote only root conversations to Tracks, preserving the accepted Track target independently of conversation ancestry.
- The existing Delegate and Send to thread actions remain the launch and provenance-preserving messaging paths. Agent guidance explains their CLI equivalents and typed result publication.
- Launch reviewer creates a fresh child Thread from the candidate branch with its objective, source checkout, integration target, and editable review brief. It receives no implementation transcript. Review findings return to the original worker; preparation still runs in the source Thread. Reviewer Threads are excluded from integration candidates and cannot be integrated as implementations. Tag-write failures are surfaced as warnings without falsely failing a successful reviewer launch.
- Handoffs exposes typed durable results separately from routine mailbox traffic, with source-thread drill-down. Reads preserve channel read markers and search each channel's latest 500 messages; the view shows at most 60 results across the 20 most recently active relevant channels. Full history remains available through the source Thread and Loom channels.
- Track-level Integrations continues to route explicit strategies to the same-repository coordinator. The queue also exposes reported stale/conflicting candidates and links to preparation/review. Stale/conflicting facts override an old readiness tag. A sleeping/stopped process or a result message alone never establishes readiness, and a queued integration request never establishes success.
- Needs You and OS notifications require explicit attention/blocked tags or live tool approvals. Errors, completion, review activity, waiting, and parent-help requests remain visible facts without automatically notifying a human.
- Thread rows and home views display advisory working, reviewing, ready, stale/conflicted, needs-parent, waiting, integrated, and resting labels. Inspector tabs scroll to keep all views accessible in a narrow rail.

The specification explicitly places automation triage Threads, lightweight model routing, event coalescing, sleeping-worker wakes, maintained mergeability checks, and richer durable Track state after this initial phase and dogfooding. These are not added as a new scheduler or control-plane service here. This phase displays reported readiness facts; it does not continuously verify candidate mergeability. Durable Track brief maintenance is agent guidance using existing versioned artifacts.

Validation:

- `npm test`: 136 passed.
- `npm run typecheck`: passed.
- `npm run build`: passed.
- `cargo test --manifest-path src-tauri/Cargo.toml --locked`: 98 passed (89 unit, 4 decode, 5 integration-target).
- `git diff --check`: passed.

Regression coverage includes independent reviewer context/source routing, legacy-server role delivery, avoiding duplicate launches after metadata failure, recursive Track ownership, stale readiness precedence, explicit human escalation, and handoff provenance. No live agent launch or deployment was performed as validation.
