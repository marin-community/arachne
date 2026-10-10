# Arachne Thread Landing and Track Publishing

A worker **Thread** owns a bounded result. A **Track** owns the accepted Git
state and its coordinator. **Land** accepts a Thread into that Track.
**Publish** sends the Track's accepted state toward a PR or a local/upstream
destination. These are separate authorizations.

## Thread Land

The worker header and the inspector's **Landing** queue expose **Land**. A
primary click uses **Squash into track** by default: one logical commit on
the Track's canonical topic branch. Merge, rebase, and selective cherry-pick
are available when appropriate. Strategy selection constrains the final
acceptance operation; it does not bypass inspection, review or validation.
Thread Land never targets primary `main`, opens a PR, or publishes upstream.

The queue includes working, waiting, reported ready, stale, conflicted and
landed outcomes. It excludes the Track coordinator and reviewer-only
Threads. Select a source to inspect/review its changes, then Land it, or
Land directly while it is still working. Process state and reported
readiness are advisory. An explicit ready tag is not required to request
acceptance, and a sleeping worker is not automatically ready.

One Land click authorizes the complete operation:

1. Queue a wrap-up message to the original source behind its active ACP
   turn, without interrupting the turn or redirecting the source to a new
   repository/branch. A terminal source receives a durable channel message;
   the app does not type into an active terminal or claim an ACP queue.
2. Deliver one complete acceptance request to the Track coordinator. It
   waits for a successful intended source result, reviews it independently
   when risk warrants, repairs straightforward issues, and validates it.
3. Commit only intended source changes, including intentional editor saves,
   while preserving foreign staged, unstaged and untracked work. Never use
   blanket add/commit or destructive cleanup to obtain an apparently clean
   source. Preserve unrelated target work as well.
4. Re-verify the original source identity, repository, branch and revision,
   and the canonical Track target and current revision. Refresh inspection
   and validation when either changes. Reconcile in isolated worktrees;
   do not hijack a coordinator scratch branch or another writer's checkout.
5. Complete the selected acceptance operation into the canonical topic ref
   without asking for a separate **Prepare** click. Record the actual commit
   and validation evidence, then stamp `integration_result` on the source.

A queued request is preparation and acceptance requested, never a landed
result. `landing_request=pending` may describe an advisory request; it must
not imply readiness or successful acceptance. Stale, conflicting or partial
work can be repaired under the original request, but a real product decision,
missing access, changed source identity or unrelated in-progress Git
operation remains an actionable blocker.

**Review → Prepare** remains optional preparation only. It inspects,
repairs, validates and commits intended source changes without accepting or
publishing. Worker review compares against the canonical Track topic branch.
Track review uses the canonical accepted ref as its candidate and the
publishing base as its comparison, even when coordinator scratch differs.
Local previews calculate that actual comparison. Loom has no arbitrary-ref
remote diff read endpoint: remote previews report their scope as unavailable
and preserve the Review action for an independent agent to inspect the real
canonical comparison in the remote repository. Never label a recorded-base
diff as the canonical comparison.
Reviewer Threads report findings and are not acceptance candidates.

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
wakeups to inspect successful complete work, delegate independent review and
repair, and accept it as a squash into the canonical topic branch. Partial,
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
success; worker `integration_result` remains distinct.

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
