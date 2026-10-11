# Projects across Loom backends

Arachne registers backends in `arachne.loomServers` and shows their accessible
fleets together. `loomUrl` is the default for future project and ungrouped track
creation. Changing that default preserves existing project associations and
the currently selected thread. Project creation and the new-track setup card
offer a backend choice; a project's **+** selects its owning backend.

A project remains a non-system Loom layout group. Its backend owns its group,
tracks, repositories and resource bindings. Filing and reparenting operate within
that backend. Moving a track to a project on another backend is rejected before
the request reaches Loom.

## Identity and routing

Normalized HTTP(S) URL is the backend identity. URLs containing credentials,
queries or fragments are rejected. Frontend session, branch, space and group
identities encode both the URL and the server-local ID, using `backendIdentity.ts`.
Resource, Todo and activity references returned by the transport are qualified
on their identity-bearing fields as well. Model IDs, config IDs and freeform
user text retain their original meaning.

`backendInvoke.ts` resolves request ownership from qualified identifier fields,
or an explicit/injected backend URL for calls such as repository lookup and
launch catalog loading. It rejects mixed-backend arguments and removes UI
qualification before invoking Rust. When adding an identifier argument, update
the argument identity field set and add a routing regression. Reply references
also need qualification before entering shared UI state.

Rust commands require `base_url` for Loom operations. `ServerRegistry` resolves
that address without a default client. Each entry owns its client, fleet
cancellation token, selected chat generation and pagination cursor. Reconnecting
one entry retires only that entry's subscriptions and stale pages. Chat events
carry `{server_url, session_id, event}`; listeners check both identities. Local
OS and `gh` commands keep their local behavior.

Explicit Change declarations, own-channel result bodies and stored acceptance
receipts retain server-local identities. The frontend validates those exact wire
values against the local portion of the qualified source session ID. A selected
Change pairs a frozen declaration with its qualified UI source ID; queue and
selection keys include backend, source and Change ID. Shared UI completion and
receipt references use the source backend's namespace, including target session
and completion message IDs, without changing stored evidence.

`change_changes`, selected `review_session`, selected preparation and
`land_change` route through that selected source's backend. The transport retains
the exact declaration and strips only UI routing identifiers. Mixed-backend
selections and canonical targets fail before dispatch. An unavailable backend
invalidates its queue selections and cannot fall back to a healthy server with
the same local IDs. Completed ranges can request coordinator acceptance even
when the original source runtime is archived; they never resume it.

Selected operations retain the owning client's availability and retirement lease
through each preflight read, route update and dispatch. An outage or client
replacement invalidates existing leases even if the URL immediately recovers;
healthy fleet refreshes preserve them. A selected Prepare carries paired
`changeId` and `expectedChange` into `send_input`, which independently rechecks
its exact declaration and snapshot on one leased client before source delivery.
Cancellation after a task or reviewer request may have reached Loom is reported
as uncertain delivery and requires reconciliation before retrying.

Connection attempts reserve a generation before network verification. Only the
latest attempt for that URL can install a client. Fleet publication is fenced
against client retirement and availability changes, so delayed mutation reads
cannot restore a stale fleet after an outage or reconnect.

Launch catalogs and remembered launch selections are scoped by URL. Named
presets remain reusable templates applied explicitly by the user. Repository
defaults and composer drafts are also backend scoped. The previous single
backend, raw project preference keys and existing composer references migrate
without recreating projects. Bearer tokens remain in per-URL Keychain entries;
frontend storage contains no credentials.

## Unavailable and recovery

The frontend preserves each backend's latest layout and excludes unavailable
sessions from actionable fleet views. Project headings remain visible with a
dimmed name, **Unavailable** label and reconnect action. Creation and project
resource mutations are disabled for unavailable backends. Restored project drafts
reload their inherited resources after recovery; launch waits for that checklist,
and failed reads offer a retry that preserves the draft. Excluded resources stay
excluded for the same project and repository when they still exist after recovery;
new bindings inherit by default. A selected unavailable
thread displays a recovery panel; healthy backend navigation remains usable.

`arachne.backendProjects.v1` caches confirmed layout metadata without session
lists or conversation contents, so known project headings survive restart.
Successful snapshots replace only the matching backend. Initial connections,
pending launches and aggregate Todos are independent across servers; a slow Todo
response cannot withhold another backend's results.
Todo request coalescing also happens per backend: a stalled request cannot block
healthy refreshes or completion, and removing a backend retires its old replies.
Completion locks belong to individual Todos; retired mutations cannot unlock
newer requests or publish their late errors.
Composer instances have their own generation, including when reopened on the
same backend; older launch completions cannot navigate, dismiss a newer composer,
or clear its saved draft. Successful background launches remove only their
unchanged submitted snapshot, so completed work is not restored on reopening.

SSE transport failures emit an unavailable transition immediately, including
authentication failures. A successful subscription emits a resync transition
even when the server has no new session events. The fleet actor cancels an
in-flight reconciliation on outage and reads a fresh snapshot on recovery.
Every required stream shard must recover before the backend becomes available.
If REST reconciliation fails while its streams remain live, it retries with a
backoff capped at five seconds; outage or retirement cancels the retry. Successful
reconciliation returns to event-driven updates without healthy idle polling.
Mutation reconciliation failures wake that backend's actor through the same
retry path. Initial snapshots, actor reads and mutation refreshes share a
publication generation: an older read cannot replace a newer mutation or
availability transition, and invalidated reads trigger fresh reconciliation.

## Regression checks

Run `npm test`, `npm run typecheck` and `npm run build`. Tests exercise colliding
IDs, migration and cached headings, mixed-backend rejection, project-owned
launches, late navigation replies, chat envelopes, notifications, catalogs and
independent Todo publication. In `src-tauri`, run `cargo fmt --all -- --check`,
`cargo check --locked --offline` and `cargo test --locked --offline` when the
dependencies are available. Rust TCP fixtures cover credential isolation,
stalled reconciliation, disconnects, authentication failures, quiet recovery,
and delayed actor or initial snapshots racing newer mutations.
