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
  "target_resource": "<upstream branch, e.g. main>",
  "strategy": "open-pr | squash | merge | rebase | push | decide",
  "requested_by": "user"
}
```

---

## Procedure

1. **Inspect source and target state.** Find the worker session's worktree
   (it is named in the request's `source_work` field when present, or
   resolvable via `loom sessions get`). An archived worker may have no
   checkout; use its durable branch and create a fresh worktree if needed.
   Check both branches' actual state —
   `git log`, `git status`, `git diff` — never assume.
2. **Verify the source result is complete.** Uncommitted changes in the
   worker's worktree are part of its result (the user may have edited
   manually in Zed — that is valid and expected). Commit them first if the
   strategy operates on commits.
3. **Identify the relevant commits/diff** the integration will carry.
4. **Update/fetch target state.** If the target branch changed since the
   worker started, reconcile rather than fail: the topic's canonical state
   is the branch, and a stale integration worktree must be refreshed before
   integrating (the user may have edited it directly too).
5. **Apply the requested integration strategy** (see below).
6. **Resolve straightforward conflicts** yourself.
7. **Run required validation** — tests, typecheck, build, whatever the
   repository's conventions require. If the repo has no obvious convention,
   run what exists.
8. **Update the target branch.** Push if the repository expects it.
9. **Record resulting commits/PRs** in your report; associate the PR if one
   was created.
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

Push the topic branch itself as the landing action.

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
  need a checkout to operate in.
- Worker hierarchy and git branch ancestry are independent. Fork new
  integration work from the **topic branch**, not from the parent worker's
  branch, unless the request says otherwise.
- After a successful integration, future workers should fork from the
  updated topic state — this happens naturally when you push the topic
  branch; nothing else is required.
- Do not delete the worker's branch or worktree after integration.
  Archival is a separate lifecycle concern.
