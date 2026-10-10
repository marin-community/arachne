# Integration skill

You are acting as the **coordinator** for an integration or landing request.
These requests arrive as a structured prompt from Arachne's **Integrate** /
**Land** buttons — but they are the same operation a user could have typed
as a sentence, e.g. *"Integrate the Zed worker into the Arachne topic."*
Treat the JSON payload and the prose statement as one intent.

The central principle (spec: `docs/integration-and-landing.md`):

> **Integration is an agent action with an explicit strategy, not a
> hard-coded git operation.**

The strategy is an **intent constraint**, not an instruction to execute one
raw git command. You remain responsible for inspecting the actual repository
state, handling conflicts, running validation, and reporting the result.

---

## Request shapes

### Integrate (worker → topic)

```json
{
  "action": "integrate",
  "source_thread": "<worker session id>",
  "source_resource": "<worker branch>",
  "target_scope": "<topic name>",
  "target_resource": "<topic branch for this repository>",
  "strategy": "squash | merge | rebase | cherry-pick | open-pr | decide",
  "requested_by": "user"
}
```

### Land (topic → upstream)

```json
{
  "action": "land",
  "source_resource": "<topic branch>",
  "repository": "<repo root>",
  "target_resource": "<resolved destination branch; primary current branch for land-locally>",
  "target_origin": "primary-checkout | main-fallback | recorded-base | explicit",
  "strategy": "open-pr | squash | merge | rebase | push | land-locally | decide",
  "requested_by": "user"
}
```

**Landing target policy.** For non-PR strategies (`squash`, `merge`,
`rebase`, `push`), the target is the **primary local checkout's currently
checked out branch** — the checkout a human actually opens — falling back
to `main` when it cannot be resolved. The primary checkout is the
repository's main working tree, not a worker worktree. `target_origin`
records how the control plane resolved the target:

- `primary-checkout`: resolved from the primary checkout's current branch.
  Re-verify it against the live checkout before writing — the user may
  have switched branches since the request was queued.
- `main-fallback`: the control plane could not read the checkout
  (detached HEAD, unreadable repo, or a scoped credential). Try resolving
  the primary checkout's current branch yourself from `repository`
  before settling for `main`.
- `recorded-base`: the branch's recorded base (the remote's default
  branch) — the normal target for `open-pr`.
- `explicit`: the user chose the target; do not substitute another one
  without asking.

`open-pr` targets the branch's recorded base. For strategies other than
`land-locally`, an explicit `upstream` override beats defaults.

**`land-locally` is stricter:** it means squash the prepared source into the
**primary checkout's currently checked out branch**, with target origin
`primary-checkout`. Re-resolve that checkout and branch immediately before
writing. It has no `main`, `main-fallback`, or recorded-base fallback and
must not land into a worker checkout. If the current branch is unavailable,
report the blocker rather than substituting another destination.

**Authorization includes preparation.** A queued Land request authorizes
review, straightforward repair, required validation, committing intended
source changes, and then completion of the chosen strategy. Continue through
the operation without a second confirmation or stopping after a readiness
report. This includes queued `land-locally` repair after dirty-source,
staged-target, or recoverable conflict detection. Review → Prepare is an
optional preparation-only request and does not authorize the landing action.
`local=false` is an acknowledgement of queued preparation and landing,
without landing details or a completion tag; report completion only after
the chosen operation actually succeeds.

---

## Procedure

1. **Inspect source and target state.** Find the worker session's worktree
   (it is named in the request's `source_work` field when present, or
   resolvable via `loom sessions get`). An archived worker may have no
   checkout; use its durable branch and create a fresh worktree if needed.
   Check both branches' actual state —
   `git log`, `git status`, `git diff` — never assume.
2. **Prepare and verify the source result.** Inspect uncommitted changes
   and their ownership/relevance, including intentional manual edits.
   Review and repair the intended result, run required validation, and
   commit only intended source changes if the strategy operates on commits.
   Preserve unrelated or foreign edits, staged changes, and untracked files
   in both source and target checkouts; report exclusions. Never use a
   blanket add, reset, or cleanup to absorb or discard another writer's work.
   Delegate substantial review or repair through visible Loom child
   sessions, including reviewers, with exact source/target references and
   typed results on their own channels.
3. **Identify the relevant commits/diff** the integration will carry.
4. **Update/fetch target state.** If the target branch changed since the
   worker started, reconcile rather than fail: the topic's canonical state
   is the branch, and a stale integration worktree must be refreshed before
   integrating (the user may have edited it directly too).
5. **Re-verify and apply the requested integration strategy** (see below).
   After preparation or delegation and immediately before writing, verify
   the exact source repository, branch and revision and target repository,
   checkout, branch, revision and origin. If either revision changed,
   refresh the diff and required validation for the actual revisions.
   Preserve the chosen strategy and destination semantics.
6. **Resolve straightforward conflicts** yourself.
7. **Run required validation** — tests, typecheck, build, whatever the
   repository's conventions require. If the repo has no obvious convention,
   run what exists.
8. **Update the target branch.** Push if the repository expects it.
   For **landing** with `squash`, `merge`, `rebase`, or `push`, honor an
   explicit destination; otherwise use the primary local checkout's current
   branch (`main` if it cannot be resolved). Resolve the checkout from the
   request's `repository`,
   re-verify which branch it has checked out now, and land there. Do not
   land into a worker worktree. The primary checkout may be dirty with the
   user's in-progress edits: integrate around them while preserving their
   content and staged/unstaged state. For `land-locally`, squash into the
   primary checkout's live current branch with **no fallback**. Use an
   isolated checkout for preparation/conflict repair where needed, then
   safely update the primary target without committing foreign target edits.
   Complete the authorized operation without asking for confirmation again;
   escalate only for judgment or access outside that authorization.
9. **Record resulting commits/PRs** in your report; associate the PR if one
   was created. For a completed worker integration, stamp the source session
   with `loom sessions tags set integration_result <commit-or-PR-reference>
   --session <source_thread> --note "<strategy> into <target branch>"`.
   Do this only after the target update or PR creation and validation have
   succeeded. A sent request or failed integration is not a result. If the
   tag command fails, report that separately; the git result still stands.
10. **Report a concise outcome** to the requesting thread (the parent
    coordinator, or the user if you are the topic leader): what was
    integrated, where, the commit hash or PR link, and validation results.

---

## Strategies

### `squash`

Incorporate the worker's effective changes as **one logical commit** on the
target branch. You may rebase/update against the target first, resolve
conflicts, run validation, then create the squash commit. Preserve the
worker's branch for history — do not delete it.

### `merge`

Preserve worker branch history and merge it into the target branch,
normally producing a merge commit where appropriate.

### `land-locally` (landing only)

Squash the prepared topic's effective changes into one logical commit on
the **primary checkout's currently checked out branch**. Re-resolve the
primary checkout, current branch and revision before the final write; never
use `main` or the recorded base as a fallback. Preserve unrelated or foreign
source and target edits, including the target's staged/unstaged state and
untracked files. Repair and validate recoverable issues, then complete the
selected local landing under the original authorization. Report the actual
target and resulting commit, or a verified already-contained no-op. A queued
repair request is not a completed landing.

### `rebase`

Rebase worker commits onto the target branch and integrate the resulting
history. Useful when linear history matters.

### `cherry-pick`

Selectively apply the worker's commits. If the worker has multiple commits
and no obvious selection exists, ask for clarification in your report
rather than guessing.

### `open-pr`

Do **not** directly modify the target branch. Create a PR from the worker
branch into the topic branch. Report the PR number and link.

### `push` (landing only)

Push the topic branch itself as the landing action. Like the other non-PR
strategies, the effective upstream target is the primary local checkout's
checked out branch — update that branch (fast-forward it to the topic
branch, or push the topic branch onto it) rather than assuming `main`.

### `decide` (Ask coordinator to decide)

Inspect the worker result and repository context, **choose** the appropriate
strategy, execute it, and explain what was chosen and why. This is the safe
fallback when policy is unclear.

---

## Failure states — distinguish these

- **Mechanical failure** (git failure, tests fail, target unavailable):
  attempt repair yourself. If genuinely stuck, report the failure concisely.
- **Conflict requiring engineering work**: keep working; spawn a child
  integration worker if the reconciliation is substantial. Do not
  immediately bother the user.
- **Conflict requiring product judgment**: **escalate** — use
  `loom status set --tag attention` and lay out the options. Never invent a
  product decision.
- **Stale source** (worker branch changed since review): refresh and
  re-evaluate before integrating.

---

## Policies

- Do not assume every repository uses the same workflow. When a repository
  or topic declares configuration (target branch, preferred strategy,
  required tests, direct-push allowed, PR required), follow it.
- One active writer per worktree: never write directly into another
  worker's checkout. Create your own worktree for the target branch if you
  need a checkout to operate in. For `land-locally`, prepare in isolation
  and coordinate the authorized final update to the primary checkout;
  preserve other writers' edits and re-verify its current branch/revision.
- Worker hierarchy and git branch ancestry are independent. Fork new
  integration work from the **topic branch**, not from the parent worker's
  branch, unless the request says otherwise.
- After a successful integration, future workers should fork from the
  updated topic state — this happens naturally when you push the topic
  branch; nothing else is required.
- Do not delete the worker's branch or worktree after integration.
  Archival is a separate lifecycle concern.
