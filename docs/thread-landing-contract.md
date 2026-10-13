# Explicit Change Land / Track Publish contract

Threads produce **zero, one, or many explicitly declared Changes**. The inspector
Changes timeline contains Changes, never inferred Thread obligations. Successful
review/research/coordination results with no outputs require no Land, regardless
of title, role tags, process status, completion, or existence of a Loom branch.
An authenticated explicitly declared implementation range remains eligible if
its source is labeled `role=reviewer`, including after a worker is relabeled.
Eligibility follows the selected output and its exact completion/scope/receipt,
never the source's current role, title, or status. Ordinary review verdicts with
`changes:[]` still produce zero outputs.
When Loom summaries omit checkout paths, the native adapter resolves only
declared-output rows through authenticated `sessions.get` on the owning client
(deduplicated raw IDs, at most eight concurrent lookups, five seconds each).
The full response must match the exact raw session, branch ID, repository and
branch before its actual path supplies identity evidence. Declarations never
supply actual identity. Missing/failed evidence disables actions as identity
unavailable; verified conflicts still require reconciliation. Existing client
retirement/outage fences also cover these reads.
Track Publish stays a separate explicit action on the canonical accepted ref.

## Durable Change v1 (implementation contract)

Use existing `sessions.tags.set` metadata: one source-owned tag
`change:<id>` per output, whose value is JSON below. IDs are stable nonempty ASCII
`[A-Za-z0-9._-]+` (maximum 80 characters), unique within the source Thread; the
qualified identity is `<source_session>/<id>`. Never recycle IDs or delete old
outputs to make a cumulative branch appear fresh. Separate tags avoid rewriting
siblings when one output changes. No new Loom endpoint or scheduler is needed.
These APIs were inspected in Loom's `operations/sessions.rs`,
`operations/artifacts.rs`, `operations/channels.rs` and channel delivery code.

```json
{
  "version": 1,
  "id": "parser",
  "title": "Parse explicit outputs",
  "source_session": "worker-id",
  "repo_root": "/path/to/repository",
  "source_branch": "weaver/worker",
  "source_work_dir": "/path/to/original-worktree",
  "scope": {
    "kind": "range",
    "base_oid": "FULL_GIT_OBJECT_ID",
    "head_oid": "FULL_GIT_OBJECT_ID",
    "paths": []
  },
  "completion": {"channel_id": "worker-id", "message_id": "result-message-id"}
}
```

`scope.kind` is `range` (committed exact base→head diff) or `working` (declared
bounded output still being produced). OIDs are full 40/64 hexadecimal object
IDs. All non-ID metadata text (title, session/message pointers, repository/worktree
strings and working version) must be nonempty after ECMAScript whitespace
validation and must exclude ASCII C0 controls U+0000–U+001F and DEL U+007F.
Validation never trims or rewrites strings. Literal paths follow the same
nonempty/control policy, exclude backslash, absolute paths, empty/dot/dot-dot
components and `.git` components, and remain EXACT strings: embedded/leading
spaces, Unicode and glob characters are supported literally; whitespace-only or
control-bearing filenames are unsupported in v1. Frontend/native validators
share `tests/fixtures/change-text-contract.json`, including receipts and sibling
registry behavior. This constraint applies to declared metadata only; foreign
files of any name must still be preserved.

`paths` is an array of repository-relative literal paths; empty means the
entire pinned range, never the entire cumulative branch against an incidental
base. For `working`, paths MUST be nonempty and `scope.worktree_version` MUST
pin the existing `session_changes.version` string; completion is null. A working
snapshot is request identity, not readiness. Finalizing working scope retains
the ID and original identity, records a range, and requires fresh scope review.
No implicit expansion to sibling outputs, ancestors, or unrelated checkout edits.
Land on a completed range retains its ORIGINAL successful completion reference:
the native command sends ONLY the coordinator acceptance request, with no source
subscription, adoption, task prompt or notification for a completed range.
It does not request another completion, reindexing, resumption or replay of the
finished task. The coordinator can review/repair in isolation.
After archival removes the linked checkout, local range validation and preview
use the declared repository's retained common Git store. Original worktree
metadata remains unchanged; missing objects/refs or an unrelated replacement
checkout fail closed. Working scopes still require the original live checkout.
For committed ranges a branch moving does not change the selected immutable
range: review that range and reverify original identity and reachability. A
changed declaration requires reselection/fresh review, never silent substitution.

The result is a typed `kind=result` message on the source's OWN durable channel,
with authenticated source provenance, stating successful intended wrap-up and
listing `changes` (zero or many) with exact IDs/scopes. Its JSON body or structured
payload has `{version:1, outcome:"complete", intent:"final", changes:[...]}`;
inline declarations carry `completion:null` because the message cannot know its
own ID before send. Extra summary/checks/review/exclusions fields are allowed.
The indexed declaration then references the returned message ID. Completion is a reference
to that message, never inference from status/tags/title or a review approval.
Workers send the result first, capture its returned message ID, then set their
output tags referencing it. Autoland wakes on the actual result and reads its
explicit outputs, re-reading tags when available; the inline declarations allow
result delivery before tag indexing. A later tag alone does not manufacture a
successful completion. Failed/checkpoint/help/partial/interrupted outcomes never
qualify. A parent must read and verify the linked original typed result before
acceptance; references are evidence pointers, not verified readiness.

Acceptance uses a separate coordinator-owned tag `change_acceptance:<id>` on
the original source, JSON `{version:1, change_id, source_session, scope,
 target_session, target_branch, target_oid, outcome:"accepted", completion}`.
It is written only after actual canonical acceptance and validation. Match the
qualified identity AND exact scope/completion; an invalid/mismatched receipt
blocks reacceptance until reconciled, rather than presenting a fresh output.
Accepting one output never updates another. Keep historical accepted entries as
landed history; repeated completion delivery and identical duplicate IDs are
idempotent, conflicting duplicate metadata fails closed. A new ID/range from an
already accepted Thread remains eligible. Exact already-contained/no-op outputs
may be recorded with verified containment evidence, never replayed as deltas.

Legacy `integration_result` is historical compatibility evidence only. It must
NEVER mark every declared output accepted, nor hide a later output. Do not
synthesize declarations from historical Threads/results/branches. To migrate an
already accepted output, explicitly declare its original bounded range and
matching per-Change receipt after verifying accepted history. Existing manual
`land_thread`/`integrate_session` requests remain explicit legacy authorizations,
with all existing completion/identity/canonical protections, but the UI must not
expose them for undeclared Threads. New selected actions use Arachne commands:
`land_change {sessionId, changeId, expectedChange, strategy}` and
`change_changes {sessionId, changeId, expectedChange}`. Both re-read the source
registry, validate exact selected declaration and original source identity, and
fail closed for stale/malformed/mismatched/accepted scope. `change_changes`
returns the existing ReviewChanges shape; range previews compare EXACT pinned
base/head and literal paths, exclude live checkout edits, and remote/unavailable
comparisons remain explicitly unavailable. Working previews verify pinned HEAD
and worktree version and include only declared paths. Review and preparation
requests include the selected declaration; avoid thread-wide prep/acceptance.
`review_session` accepts optional paired `changeId` and `expectedChange` inputs:
when provided, re-read/validate the exact selected registry before launch and
embed its immutable scope in reviewer context. Range reviews authenticate the
original final completion; working reviews explicitly have null/unverified
completion, verify HEAD/version/paths and report read-only coverage without
requiring a final result or claiming readiness. Acceptance after finalization
still requires fresh exact range review and authenticated intended completion. Existing Track and explicitly
unselected legacy review callers remain compatible. Selected preparation
preflights `change_changes` before sending its bounded original-source prompt;
stale identity/scope fails before dispatch.
Its `send_input` also carries paired `changeId`/`expectedChange`; native code
reauthenticates and rechecks on one owning-backend availability/retirement lease
before lifecycle/input delivery. Its fresh runtime view must match the original
identity and exact registry/receipt plus working HEAD/version before adoption;
successful or contested adoption is revalidated before input. A preview from a
replaced client cannot authorize delivery without that fresh exact native preflight. Outage/replacement cancels
the old lease through recovery; possibly dispatched requests require reconciliation.

With concurrent backends, `sessionId`/`sourceId` in UI selections are qualified
by the owning backend URL. The selected declaration, persisted tags and typed
result bodies remain exactly server-local, including `source_session` and
completion channel/message IDs. Queue keys pair the qualified source with the
Change ID; receipt and completion navigation references are projected into that
same backend namespace. Each native Change/review command requires explicit
`base_url` ownership. No active-server fallback or cross-backend Track resolution
is permitted, even for identical local IDs and repository paths.

Land on a working output queues a noninterrupting selected-output wrap-up and
one complete coordinator request. Completed ranges already have authenticated
final evidence and deliver only the coordinator request; source runtime state
or capacity must not be needed for their acceptance. It carries the pinned declaration through retry identity,
review, repair and acceptance; no second Prepare click. Immediately before EVERY
acceptance write re-read same-repository live owning Track, canonical ref and
initialization, original source session/repository/branch/worktree and selected
scope/completion/receipt; refresh review on changed scope or target. Preserve
foreign staged/unstaged/untracked/ignored files. Default squash accepts only the
selected delta; merge/rebase cannot drag siblings or already accepted history.
Autoland uses eligible explicit outputs from real durable result wakes with live
policy rechecks and authenticated interruption protection, never automatic
resumption, polling, publishing or primary/parent checkout writes.

The pinned Git scope itself is immutable review evidence. Detailed scope/review
reports may use existing branch-scoped artifacts with immutable revision refs;
`artifacts.write --base-rev` guards updates. Metadata is agent-owned advisory
indexing, not a transactional Git acceptance engine: agents perform actual Git
validation and acceptance and report receipts only after success.

## Durable branch tags (Loom metadata)

- `topic=true`: Track coordinator marker (existing).
- `track_topic_branch=<valid local branch ref>` on the Track coordinator: canonical accepted ref, distinct from its Loom-managed scratch/session branch. Missing tag on a legacy Track means its existing coordinator branch; never silently migrate legacy work.
- `track_topic_initialized=<canonical ref>`: only the initialization agent stamps this after exclusive Git creation and seed verification. New Tracks retain their initialization goal marker and native acceptance/publishing requires matching canonical/verified tags; early launch display tags cannot authorize an existing unrelated ref. Legacy Tracks without the marker keep their existing behavior.
- `track_autoland=on|off` on the Track: OFF when absent/invalid.
- `thread_autoland=inherit|on|off` on sources: INHERIT when absent/invalid. ON overrides Track OFF; OFF overrides Track ON. Only complete successful intended results qualify for automatic acceptance; checkpoint/help/partial/interrupted work does not.
- `landing_request=pending` on a source may record advisory queued wrap-up/acceptance. It is neither readiness nor a successful result.
- `integration_result` remains historical compatibility evidence for actual successfully accepted legacy work, never a blanket receipt for declared outputs; `integration_state`/`integration_ready` remain advisory reported state, never inferred from sleeping/working/completion.
- Track publishing records `publish_result` after actual successful publish, separate from worker acceptance outcomes.

Track chat keeps the canonical source branch and historical `publish_result`
under a native, collapsed-by-default "Publish details" disclosure, labeled
"Source branch" and "Previous publish". Historical metadata does not describe
the current candidate or request. Explicit Publish controls and transient
busy/result/error feedback remain visible.

## Canonical ref creation

New Tracks have an editable topic-branch field; empty means automatic `arachne/tracks/<slug>` inferred from explicit title or launch title/task. Draft persists the editable value. Validate explicit local branch refs before launch; reject invalid/reserved ref forms. Disambiguate duplicate defaults and reject explicitly occupied/claimed unrelated refs, preserving existing branches. Initialize the REAL Git canonical ref from the fresh Track seed revision (launch HEAD, before substantive work), through an explicit durable initialization instruction embedded in the launch prompt; safe local creation is optional. Never rename a live Loom branch or change primary HEAD. Tags must agree with the created ref. Remote initialization must do real Git work and report its result; display metadata alone is not initialization. Canonical initialization is required before any acceptance/publishing. Legacy Track without a tag retains its existing branch.

## Changes timeline and serial merge queue

The Track's **branch-scoped** `change-queue.json` artifact owns durable pending
order and authorization. It belongs to the coordinator's Loom branch ID, even
when the canonical accepted Git ref differs from its scratch branch. It is not
repository-shared. Never store authoritative order in localStorage or replace
it through blind tags writes. Wire declarations retain raw server-local source
and channel IDs; fleet qualification is a frontend display concern and must not
rewrite original identity, completion or receipt evidence.

```json
{
  "version": 1,
  "track_session": "track-id",
  "repo_root": "/path/to/repository",
  "target_branch": "arachne/tracks/topic",
  "active": null,
  "pending": [
    {"change": "EXACT_DURABLE_CHANGE_V1_OBJECT", "strategy": "squash", "requested_by": null}
  ]
}
```

The `change` placeholder above is an object with the exact Change v1 fields.
`requested_by` is null, `user` or `autoland`. Active is one QueueEntry or null;
it records an actual coordinator claim of one completed output's acceptance,
including review/repair, never a source's running/sleeping/status. Landed history
comes from exact original receipts and actual Git evidence, never queue state.

Loom APIs were inspected in `weaver-api/src/operations/artifacts.rs` and
`loom/src/web/artifacts.rs`: `artifacts.get`/`artifacts.write` use
`branch=<Track Loom branch ID>`, `name=change-queue.json`, `repo=false`.
Reads return `meta.branch_id`, `meta.rev` and text `content`; validate the exact
branch owner, positive JavaScript-safe integer revision and queue
Track/repository/canonical identity.
A missing artifact (404 only) returns `{queue:empty initialized state,revision:0}`
without writing. Missing fields, invalid strategies/authorization, duplicate
qualified identities, malformed declarations, repository-shared artifacts,
conflicting canonical metadata (including another live owning Track claiming the
same repository/ref) and unavailable ownership fail closed.
Atomic server CAS is a required backend capability, not a version assumption.
Before every queue mutation (claim/order/promotion/clear/drain), source
lifecycle/wrap-up or coordinator instruction/wake, issue an authenticated
**GET `/api/openapi.json`** on the selected owning client and require exact
boolean `true` at `paths["/api/artifacts/write"].post["x-loom-atomic-base-rev"]`.
Missing/false/string/malformed/wrong-path/unreachable evidence fails closed.
An unsupported Loom is an actual backend update prerequisite, not missing source
completion or human attention. Only transaction-level server CAS serializes
independent coordinator writers; local mutexes, retries or post-write receipt
checks cannot repair the earlier non-atomic guard. Never infer support from a
version, cache, prior success or reconnect/replacement. Use this client's own
auth and current availability/retirement/cancellation lease, with no active-server
fallback. Native gates apply before route/delivery setup, again immediately
before artifact writes and source/coordinator dispatch, including saved-intent
or no-op delivery retry. Queue reads remain read-only; they never authorize a
mutation on an unsupported backend. Capability failure after a supported saved
promotion retains the precise queued-but-undelivered/reconciliation response.
Coordinator claim/clear/drain must perform the same LIVE gate before each CAS.
A healthy selected lease does not prove that the same endpoint still supports
atomic CAS. Runtime, adoption, contested-adoption and route receipts can
invalidate an earlier gate. Queue-specific source delivery rechecks after runtime
lookup immediately before adoption, after adoption or contested-adoption reads
immediately before the ACP prompt, and after terminal route setup immediately
before the durable message. Coordinator delivery rechecks after its final route
setup immediately before message/status dispatch, then separately before the ACP
wake after the durable instruction. No network/helper read may intervene between
the final capability check and its guarded dispatch. Ordinary nongated chat
delivery retains its existing behavior.

Writes use `kind=json` and **`base_rev=<expected revision>`**, including 0 to
guard creation. Stale state returns 409. Native mutations never blindly retry a
stale order or promotion: refresh, disclose the conflict, then retry against the
observed state. A possibly saved write with an invalid/missing receipt requires
refresh/reconciliation. No new Loom endpoint, server or scheduler is involved.

Native commands require explicit owning `base_url` and use the same selected
client retirement/outage lease as exact Change actions:

- `change_queue {topicId}` returns `{queue,revision}` from the selected Track.
- `reorder_change_queue {topicId,expectedRevision,order:DurableChange[]}` requires
  the complete exact current pending set plus eligible newly visible explicit
  declarations. It rejects active/history, duplicates, altered same-ID scope,
  foreign Track/backend/repository sources and stale revisions. Before saving it
  authenticates fresh source registry/ownership/identity/completion and working
  snapshot evidence. Existing strategy/authorization and active are preserved;
  new entries receive squash and null authorization. Reordering never means Land.
- `land_change {sessionId,changeId,expectedChange,strategy}` retains its existing
  IntegrationTarget response. After exact selected declaration, successful
  original range completion, local scope and live canonical initialization
  validation, revision-CAS promotes that one exact pending entry to index 0,
  with `requested_by:user` and the chosen squash/merge/rebase/cherry-pick strategy. It preserves active and all
  other entries. Another active entry does not prevent Land. Retrying an identical
  saved intent does not rewrite the artifact; a matching active claim is also
  preserved rather than reinserted. Scope conflicts require reconciliation.

Land saves intent before sending a durable coordinator instruction through an
installed own-channel route. Terminal coordinators use idempotent message-kind
inbox delivery. Ordinary Loom channel messages interrupt ACP turns, so ACP
coordinators receive an idempotent status-kind instruction (no automatic runtime
delivery) followed by the existing noninterrupting `queue_prompt`, `send_now:false`,
wake. Prompt receipts have no idempotent identity; duplicate wake hints cannot
duplicate acceptance because live CAS claims and exact receipts govern it.
The exact declaration and original completion supply stable retry identity;
queue revision/activity is not a new authorization. The payload is a hint: the
coordinator must reread authoritative live queue order. Working sources retain
noninterrupting bounded wrap-up/typed result routes; completed ranges receive
no source lifecycle operation, prompt, notification, subscription or reindexing.
Post-save delivery failure reports the saved queue revision, failed/uncertain
stage, absence of confirmed acceptance and identical-request delivery retry.
Uncertain ACP source or coordinator prompt delivery requires reconciliation
before retry, since that API lacks idempotent prompt identity. Queueing is not acceptance.

Only the coordinator drains the queue:

1. Read LIVE artifact, owning Track/canonical initialization, source identity,
   declarations, original authenticated intended complete/final results,
   receipts, interruptions and policy. Register new authenticated explicit
   outputs deterministically after existing order with revision-CAS, preserving
   existing authorization. Repeated exact accepted outputs require receipt and
   containment verification, become no-op history, and are never reapplied.
2. When active is null, choose the first eligible authorized pending entry.
   Manual `user` intent stays manual under autoland Off. `autoland` and null
   unrequested entries require LIVE effective autoland ON and genuine intended
   complete/final source success. Declaration, status or reordering alone is
   neither authorization nor readiness. Failed/unready intent stays durably
   pending with clear parent-owned status. Waiting for intended source completion
   is unclaimed; review/repair of an already claimed completed output retains its
   active slot. Explicit user interruption requires coordinator judgment, never
   autoresume. Review/checkpoint/help/partial/failed results do not qualify.
3. Claim ONE exact entry by a fresh expected-revision CAS setting active and
   removing only that entry from pending. Exactly one active acceptance per
   canonical ref. On conflict reread and recompute. UI order and promotion never
   preempt active. If active exists, continue only its acceptance or report its
   blocker; do not start a second writer. Working finalization requires original
   ID/title/source identity/base/literal paths, pinned HEAD reachability and fresh
   exact range review; CAS replace only the matching pending declaration,
   preserving strategy/authorization and concurrent edits before claiming it.
4. Before EVERY automatic Git/index/ref/worktree write and resumption, reread
   LIVE owning Arachne Track/source policy, identity, canonical/source revisions
   and authenticated interruption controls. Effective Off defers writes;
   explicit user interruption requires coordinator judgment, never automatic
   resumption. A parent `sessions.get` 403 may use permitted LIVE
   `sessions.list(history:true)`, never borrowed credentials or cached policy.
   Before EVERY acceptance write, reread LIVE queue and verify the exact matching
   active claim, policy for automatic work, fresh source registry/ownership/
   identity/scope/original completion/receipt, same-repository Track, canonical
   initialization and exact canonical target revision. Use isolation and preserve
   all foreign source/target edits and coordinator scratch. Queue state cannot
   manufacture successful Git writes or receipts.
5. After real successful canonical acceptance and validation, record only the
   original exact source receipt and a typed own-channel actual outcome with
   SHAs/checks. Reread queue; CAS clear only matching active while preserving
   concurrent pending edits. If receipt/result succeeds but clear conflicts,
   reconcile live state without replaying Git. Explicit reconciled coordinator
   cancellation may release a blocked claim, preserving unresolved intent; UI
   promotion cannot. Then reread LIVE queue, canonical ref, policy and source
   evidence and claim the next eligible authorized entry within the same drain.
6. Actual source/reviewer/repair result delivery drives subsequent processing.
   Verify own/immediate-parent result wake routes before idle, save parent-owned
   status and end the turn. No polling, scheduler, publication or primary checkout
   writes. Queue requests and status remain hints until real validated outcomes.

## Backend commands consumed by frontend

- `launch_session`: existing arguments plus `topicBranch: string|null` (Rust `topic_branch: Option<String>`). Used only for a new root Track (`asTrack=true`). Empty/omitted requests automatic inference; nonempty is explicit user branch.
- `land_thread`: `{sessionId, strategy}` with `squash` default; allowed acceptance choices squash, merge, rebase, cherry-pick (ask may remain compatibility). Returns existing IntegrationTarget `{coordinator_id,target_branch,coordinator_name}`; optional warnings may describe delivery limitations. `integrate_session` may remain an alias with identical acceptance semantics. No PR/push/local-main acceptance strategy.
- `set_autoland`: `{sessionId, policy}` where a Track accepts on/off and a source accepts inherit/on/off. Returns `{session: SessionView, warnings: string[]}`; a confirmed saved policy remains visible if later route/delivery/refresh setup fails. Persists Loom tag, ensures real delivery subscriptions for existing nested descendants, sends explicit complete coordinator policy instructions through existing durable delivery. Disabling policy requires stale automatic requests to reread LIVE Track/source tags immediately before writing and stop when effectively OFF. New child launches under enabled policy arrange route/notify coordinator as part of launch setup.
- `publish_track`: `{sessionId,strategy,upstream?:string|null}` returning LandResult `{local,landing?}`. Strategy wire values open-pr (Single PR), pr-stack (PR stack), land-locally (squash into local primary current branch), squash/merge/rebase/push where appropriate. Always source the canonical accepted topic branch. Existing `land_topic` may remain a compatibility wrapper, with these publishing semantics. Local accepted-ref publishing preserves foreign edits and clean synchronous behavior; coordinator scratch dirt is not part of accepted source. Queued publish returns local=false and no premature completion. Publishing requires this explicit chosen user action; no autoland path invokes it.

## Delivery and prompts

Working selected Change Land sends an explicit source wrap-up prompt behind an active ACP turn (`queue_prompt`, send_now=false), preserving original source id/repo/branch/worktree. It also persists one queue promotion and sends a durable coordinator queue wake: wait for successful intended wrap-up result, inspect/review/repair, commit intended source changes while preserving foreign edits, validate, reverify exact source and canonical target, apply chosen acceptance, and report actual outcome. Never interrupt active work or ask for a second Prepare click. For working outputs both delivery payloads carry the same selected declaration, scope and completion, and capture the same source_request_revision and optional source_request_version from the existing session_changes API for stable retry identity; this snapshot is advisory authorization identity, never readiness. Completed ranges send only the coordinator request, pinned to the declared immutable head_oid and original completion, with no source task delivery or lifecycle operation. Missing pinned HEAD blocks dispatch. Terminal sources/coordinators use durable channel messages (`send_to_thread`) instead of typing into an active terminal or claiming an ACP queue.

Use real existing Loom `POST /api/channels/subscription/set` delivery records. Direct child results already link/wake immediate parent. For nested candidates, subscribe canonical coordinator (session subject) in deliver mode to the IMMEDIATE-PARENT channel that receives linked result notices; verify returned channel_id, subject_kind=session, subject_id and mode. Ensure coordinator own-channel route as needed. Manual acceptance and autoland policy setup establish routes before claiming delivery/going idle. Future nested launch setup must establish the same route when effective autoland is ON. Use existing result wakeups/instructions; no scheduler/polling loops.

Publishing prompts clearly identify canonical source and any source checkout (which may differ from coordinator scratch). Stack mode must derive an ordered list from accepted history/branches, explicit dependent PR bases and head refs; validate each dependent change, create actual separate PRs and report ordered links/results. It may not pretend one PR is a stack.

Frontend helper `src/trackPolicy.ts` should centralize tag lookup, canonicalTopicBranch, same-repository Track resolution, effective autoland, branch inference/validation, and share these semantics across queue, ThreadView and review requests. Backend helper module mirrors this contract with cross-language fixture tests where practical. Review compares against canonical ref, never incidental scratch or main.
