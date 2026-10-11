# Arachne Change Landing and Track Publishing

A **Thread** produces zero, one, or multiple explicitly declared **Changes**.
A **Track** owns accepted Git state and its coordinator. **Land** accepts one
selected Change into that Track. **Publish** sends the Track's accepted state
toward a PR or a local/upstream destination under separate explicit authority.

## Change Land

The inspector's **Landing** queue contains declared Changes, each with a stable
source-qualified ID, title, original repository/branch/worktree, pinned scope,
completion reference and independent acceptance outcome. See
[Change v1 contract](thread-landing-contract.md) for exact durable metadata.
No output is inferred from status, title, completion, role or having a Loom
branch. A review/research/coordination Thread with zero outputs has no Land
obligation, including an unmarked reviewer. Historical completions are not
new deltas. Legacy `integration_result` remains historical evidence and cannot
accept every output or hide later outputs from the same source.

Select a declared Change to inspect its exact scope and Land it. Multiple
outputs from one Thread remain independently selectable and acceptable.
**Squash into track** is the default. Merge/rebase/cherry-pick are allowed only
when they preserve the selected delta without importing siblings or replaying
accepted history. Range previews pin the declared base/head and literal paths,
excluding live checkout edits. Working scopes pin HEAD, worktree version and
nonempty intended paths; the snapshot is request identity, never readiness.

One click authorizes the complete operation:

1. For a working output, queue the selected-output wrap-up behind an active
   ACP turn without interruption. Terminal sources receive a durable channel
   message. Completed ranges need no source task prompt, adoption or notification;
   their already authenticated original completion remains pinned.
2. Deliver the selected declaration and complete acceptance request to the
   live same-repository owning Track. Wait for authenticated successful
   intended own-channel completion covering this exact output. Checkpoint,
   help, partial, interrupted and review-only outcomes never qualify.
3. Review, repair, commit only intended source edits and validate. Preserve
   foreign staged, unstaged, untracked and ignored files in every checkout;
   never blanket add/commit or destructively clean. Use isolated reconciliation.
4. Immediately before every acceptance write, re-read the owning Track,
   canonical ref/initialization, original source identity, selected scope,
   completion and receipt, and current target revision. A changed declaration
   requires fresh selection/review; a working scope finalized under this
   request requires fresh review. Never silently expand the selected scope.
5. Apply only the selected delta to the canonical accepted ref without another
   **Prepare** click. Write `change_acceptance:<id>` on the original source only
   after actual acceptance and validation, with exact scope/completion and
   target SHA. Accepting one output never marks siblings or the Thread accepted.

Pending requests are advisory, never successful results. Changed/stale work
can be repaired under the bounded authorization with fresh review; identity,
ownership, access, genuine product decisions and unrelated Git operations can
block it. Repeated delivery is idempotent per qualified output/scope. Keep
historical accepted outputs and verify containment before reporting no-op;
do not synthesize outputs from historical branches or completion messages.

**Review → Prepare** remains optional selected-output preparation only, with
no acceptance or publication. Track review uses the canonical accepted ref
and publishing base. Remote pinned previews explicitly report unavailable
scope because Loom has no arbitrary-ref remote diff endpoint; an independent
agent can inspect the actual scope on the repository host. Reviewer Threads
hand their verdict to the parent with `changes:[]`; successful review alone
produces no Change, requests no Land and requires no human attention.

## Canonical Track topic branches

New Tracks use `arachne/tracks/<slug>` by default. The creation chat exposes
an editable topic-branch field and persists it with the draft. Slugs are
inferred from the Track title or launch title/task; explicit refs are
validated. Duplicate defaults are disambiguated. An explicitly occupied or
claimed unrelated branch cannot silently combine two Tracks.

Loom still owns the coordinator's session branch, normally `weaver/<slug>`.
Arachne records the acceptance ref separately in the durable
`track_topic_branch` tag. Before substantive Track work or acceptance, the
coordinator's explicit initialization instruction creates the **real Git
ref** from the fresh launch seed revision and reports it. Displaying a tag
alone does not initialize a branch. Initialization must preserve existing
refs, primary HEAD and the live Loom-managed branch; remote Tracks receive
a real initialization instruction as well. No remote create-branch endpoint
is invented.

An established Track without `track_topic_branch` retains its existing
coordinator branch as its accepted ref. Legacy work is not silently
migrated. Same-repository Track ancestry selects the coordinator, but its
canonical tag selects the Git acceptance target; conversation ancestry is
not Git ancestry. Child work should normally fork from the canonical
accepted topic ref unless an explicit intermediate base is intended.

## Autoland

New Track initialization retains its durable goal marker and sets `track_topic_initialized` to the canonical ref only after exclusive Git creation and seed verification. Acceptance and publishing fail closed until that verification matches the canonical tag; launch display tags alone cannot authorize a colliding unrelated ref.

Autoland is durable coordinator policy, not a browser-only preference or a
new background scheduler. It defaults **OFF** on the Track. Each source can
**Inherit**, force **On**, or force **Off**. A source override wins over the
Track setting. Turning Track-wide policy off leaves explicit source On
overrides enabled; turning a source off suppresses its automatic acceptance.
Manual Land remains a separate explicit authorization.

Policy changes persist `track_autoland=on|off` and
`thread_autoland=inherit|on|off` in Loom tags and deliver explicit coordinator
instructions. Show the saved policy even when later route setup or delivery
fails, and report the setup warning separately. The coordinator uses existing typed results and delivery
wakeups to inspect eligible explicitly declared Changes in successful intended completions, delegate independent review and
repair, and accept each selected output as a squash into the canonical topic branch. Partial,
checkpoint, incomplete or parent-help results are not completion/readiness.
Review findings alone are not an implementation to accept.

Immediately before **every automatic target write**, re-read the live Track
and source policy, source identity/revision, canonical target and result
status. An old queued instruction is not authority after effective policy
becomes Off. If policy or target metadata cannot be read, do not guess.
Autoland never opens a PR, pushes upstream or publishes to primary `main`.

Interruption notices are ordinary system-authored parent-channel messages, never typed completion results. Inspect notice context and authenticated actor provenance: partial, checkpoint and interrupted work is incomplete and requires coordinator judgment. An explicit user interruption never authorizes automatic child resumption or acceptance, including stale queued autoland requests or earlier complete results; retain the pause until appropriately authorized and reverified. Session-authored interruptions must not be described as user interruptions. Do not infer completion or readiness from the notice. Loom owns parent notification; do not send a duplicate from the Arachne client.

Direct children's typed results notify/wake their immediate parent. Nested
sources require a real deliver subscription from the Track coordinator to
the immediate-parent channel that receives those linked results (or a
verified durable forwarding route). Policy setup and future enabled child
launches establish this route through Loom's existing channel subscription
API and verify the session subject/mode. A prose instruction to wait is not
a delivery route. Before becoming idle, verify child parentage, an open own
channel/delivery binding, and the obligation to publish typed results. Save
state, set `ok` waiting status, and end the turn. Never poll fleet/channels
or keep the turn alive with timed sleeps.

## Explicit Track Publish

The Track header exposes **Publish**, with separate chosen actions:

- **Single PR** (`open-pr`): one PR for the canonical accepted topic state
  against the recorded base or explicit destination.
- **PR stack** (`pr-stack`): derive an ordered list of accepted changes from
  canonical history/accepted branches. Define each head ref and dependent
  base explicitly: the first targets the upstream base, subsequent PRs
  target the previous stack head. Validate each change and the combined
  stack. Create separate actual PRs and report their ordered URLs, bases,
  heads and validation; one PR is not a stack. If there is only one
  meaningful layer, report that honestly rather than claiming multiple PRs.
- **Squash into local checkout** (`land-locally`): squash the committed
  accepted topic ref into the primary checkout's **current** branch, often
  local `main`. It is loopback-only. It has no assumed `main` or recorded-base
  fallback, and must not act on a worker checkout. A detached/unavailable
  primary, self-target or unrelated Git operation is a blocker.
- Squash, merge, rebase or push toward the chosen external/local destination
  where repository policy supports them. An explicit destination wins;
  otherwise non-PR strategies resolve the primary current branch, with the
  existing `main` fallback when it cannot be read. Resolve and verify that
  policy immediately before writing.

All publishing requires the user's explicit chosen action. Its source is
always the **canonical accepted topic ref**, never incidental coordinator
scratch work. Prepare/validate the accepted state and continue the chosen
operation under that authorization. Keep unaccepted source/scratch edits
separate; do not silently incorporate them in a publish.

The clean local squash remains deterministic and synchronous. Preserve
unrelated unstaged target edits and untracked files, including ignored-file
collisions. Pin the accepted source SHA and reverify live canonical metadata
and the primary branch/revision/status immediately before writing. Changed
state queues fresh review under the same chosen Publish authorization; it
never silently publishes a different snapshot. A staged target must
not ride along in the squash commit. Queue coordinator reconciliation for
target staging or a safely unwound recoverable failure, with exact accepted
source and primary target snapshots. Failed Git attempts must be cleaned up
safely before dispatch; cleanup failures remain actionable errors. Do not
discard staged/unstaged content or alter another in-progress Git operation.
A completed Git operation stays completed if a later revision read or tag
delivery fails; report the known result and evidence limitation separately.

`LandResult.local=true` is compatibility wire naming for an actually
completed synchronous local **publish**, with commit/target/count or a
verified already-contained no-op. Queued publishing returns `local=false`
with no completion detail. The UI says publishing was requested; it never
claims queued work was published. Record `publish_result` only after actual
success; per-Change acceptance receipts and legacy `integration_result` remain distinct.

## Evidence and boundaries

Completion, sleeping and readiness are different facts. Validation and
mergeability evidence must identify the source and current canonical target
revisions. Never set `integration_ready` solely because an agent finished.
A newly advanced target requires fresh inspection/revalidation; a result
message is not control-plane verified readiness. Preserve branches and
worktrees for history; archival is separate.

Coordinators and workers delegate substantial implementation, review and
repair through visible Loom children with explicit repository/base and
bounded briefs. All handoffs are typed results on the child's own channel.
The parent owns acceptance and synthesis; human attention is reserved for
judgment or access the hierarchy cannot resolve. Ordinary repair, completion
and waiting are `ok` states, not Needs You.

The exact metadata and backend command contract lives in
[thread-landing-contract.md](thread-landing-contract.md). Agent procedure is
in [integration guidance](../skills/integration.md); role/wake guidance is
in [coordinator](../skills/coordinator.md) and [worker](../skills/worker.md).

Dispatch receipts confirm request delivery only. A malformed or lost receipt
can mean the full authorization already arrived: report uncertain delivery
and reconcile the durable thread/channel before retrying. Terminal requests
use stable identities and verified own-channel delivery subscriptions; they
never type into active shells or claim an ACP queue.

Delegated review handoffs belong to the immediate parent: ready for review,
approval, requested changes, failed checks, conflicts and requests for context
remain `ok`. Publish a typed own-channel result to wake that parent; a concrete
coordinator help request may also use `parent_help=requested`. Never use
`attention` or `blocked` merely to wake an agent. Intermediate parents resolve
or forward the request through the hierarchy. If a child accidentally raises
human attention for agent-owned work, its parent takes ownership, routes the
repair/review and has the child clear that status. Only a specific decision or
access requirement the hierarchy cannot resolve, or human judgment of a
concrete prepared result, warrants human attention.

PR-stack preflight accepts local or unambiguous remote-tracking upstream
comparisons, including a remote-only `main` or `origin/main`. Missing or
ambiguous local upstream snapshots queue complete coordinator resolution and
validation under the same chosen Publish action. The coordinator must verify
the actual remote PR repository/base branch before deriving dependent layers;
canonical accepted sources remain local Track refs. No queued plan is a PR.
