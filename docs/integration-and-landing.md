# Arachne Integration and Landing Spec

## Purpose

Arachne should make it easy to take work produced by one Thread/worker and
incorporate it into its Topic's accepted state without requiring the user to
manually manage branches, worktrees, cherry-picks, merges, or PR creation.
Recursive integration through a parent Thread remains possible, but the normal
user-facing flow is worker → Topic.

The central idea is:

> **Integration is a structured operation with an explicit strategy and a
> deterministic fast path for clean mechanical work.**

The UI provides **Integrate** and **Land**. Loom may complete a clean,
unambiguous integration without an agent turn; it wakes the appropriate worker
or coordinator when the operation needs repair or judgment.

Validation and the resulting commit/PR are verified by the control plane.
Agents handle conflicts, failed validation, ambiguity, and product decisions.

---

# Terminology

## Topic

A durable unit of work.

A Topic may contain:

- a coordinator Thread;
- worker Threads;
- Resources;
- Todos;
- subscriptions/events;
- one or more repositories.
- integration candidates, queue, and outcomes.

For each code repository, a Topic may define a **topic branch/ref** representing
accepted/integrated state. It has no single universal branch across repos.

Example:

```text
Topic: Arachne

repository: arachne
topic branch: arachne/dev
```

## Thread

A conversational/execution context.

A worker Thread may create commits on its own branch/worktree.

## Integrate

Move accepted work from a child scope upward into its parent scope.

Usually:

```text
worker branch
    ↓
Integrate
    ↓
topic branch (or explicit parent scope)
```

Integration may be recursive, but worker → Topic is the default visible path.

A child worker may integrate into its parent worker, which may later integrate into the Topic.

## Land

Move the Topic's accepted state into an external/upstream target.

Usually:

```text
topic branch
    ↓
Land
    ↓
PR or main/upstream branch
```

Integrate and Land are intentionally distinct concepts.

---

# Product principle

Do not make users perform routine git bookkeeping.

A worker should usually be created in an isolated worktree automatically.

When it finishes, the Thread should expose:

```text
✓ Zed remote integration

3 commits
tests passing

[Review] [Integrate ▼]
```

The user may inspect the result or immediately integrate it.

The coordinator may also choose to integrate workers autonomously when policy allows.

The Topic view should show its integration queue: each candidate's source,
target repo/ref, readiness and conflict status, validation, and outcome. The
worker card can expose the same candidate without owning the queue.

---

# Integrate UI

Use a split button:

```text
[ Integrate ▼ ]
```

## Primary click

The primary button executes the currently selected/default integration strategy.

Example:

```text
[ Integrate ] [▼]
```

If `Squash into topic` is the current default, clicking **Integrate** invokes that strategy immediately.

## Dropdown

The dropdown should expose applicable strategies.

Initial set:

```text
Squash into topic
Merge into topic
Rebase onto topic
Cherry-pick commits
Open PR into topic
Ask coordinator to decide
```

Only show strategies that make sense for the current Resource relationship.

For example, a worker with no commits should not offer `Cherry-pick commits`.

---

# Default strategy

Integration strategy should be remembered.

Suggested precedence:

1. explicit Topic integration configuration;
2. last-used strategy for this repository within this Topic;
3. last-used strategy for this repository generally;
4. repository convention/configuration;
5. conservative fallback: `Ask coordinator to decide`.

Do **not** use one global integration preference for all repositories.

Example:

```text
Arachne repo
default: squash

Loom repo
default: merge
```

The app should remember this naturally after use.

---

# Integration strategies

The strategy selected by the user is an **intent constraint**, not an instruction to execute one raw git command blindly.

## Squash into topic

Intent:

> Incorporate the worker's effective changes as one logical commit on the target branch.

The coordinator/integration skill may:

- inspect the worker diff;
- rebase/update against target if necessary;
- resolve conflicts;
- run validation;
- create the squash commit;
- update the target branch.

## Merge into topic

Intent:

> Preserve worker branch history and merge it into the target branch.

Normally produces a merge commit where appropriate.

## Rebase onto topic

Intent:

> Rebase worker commits onto the target branch and integrate the resulting history.

Useful when linear history matters.

## Cherry-pick commits

Intent:

> Selectively apply the worker's commits to the target.

The integration skill may ask for clarification if the worker contains multiple commits and no obvious selection exists.

## Open PR into topic

Intent:

> Do not directly modify the target branch. Create a PR from the worker branch into the topic branch.

Useful for repositories where review should happen through GitHub.

## Ask coordinator to decide

Intent:

> Inspect the worker result and repository context, choose the appropriate integration strategy, execute it, and explain what was chosen.

This should be the safe fallback when policy is unclear.

---

# Integration is a structured operation

The UI button and a conversational request should resolve to the same
structured operation. Neither route requires an LLM turn for a clean,
mechanical integration.

These should be equivalent:

```text
[Integrate]
```

and:

```text
"Integrate the Zed worker into the Arachne topic."
```

The underlying request should be structured.

Example conceptual payload:

```json
{
  "action": "integrate",
  "source_thread": "thread-zed-integration",
  "source_resource": "worktree-zed",
  "target_scope": "topic-arachne",
  "target_resource": "repo-arachne-topic-branch",
  "target_revision": "789abc",
  "strategy": "squash",
  "validation_policy": "topic-required-checks",
  "requested_by": "user"
}
```

Exact API/schema is implementation-defined.

## Candidate readiness and preflight

A worker stopping is not enough to make its result Ready. A coding worker
should normally stabilize/commit changes, run required validation, summarize
the result, and preflight mergeability against the current Topic ref. The UI
can then show `Ready · clean against Arachne@789abc`. Readiness records the
target revision and validation evidence; it is not a timeless property of the
worker branch. The worker can sleep while the candidate remains Ready.

Loom should deterministically re-run conflict preflight for sleeping Ready
candidates when Topic state advances. A clean candidate records the new target
revision and stays asleep. A new
conflict becomes an Event, with the original worker preferred for reconciliation
against the updated ref. Worker lifecycle and integration lifecycle are
separate: sleeping, running, and archived are not synonyms for ready, stale,
conflicting, integrated, or failed. A `git merge-tree` preview or equivalent
can surface likely conflicts before the user chooses Integrate; applying the
strategy in the disposable worktree and validating it remain authoritative.

## Deterministic fast path

For a clean candidate with a configured strategy and validation policy:

1. Create a disposable integration worktree at the expected Topic revision.
2. Apply the selected strategy there.
3. Run required validation and record evidence.
4. Advance the Topic ref only if its expected revision still matches.
5. Record the resulting commit/PR and clean up the temporary checkout.

An unsuccessful attempt leaves the canonical checkout and ref intact. If the
ref moved, refresh preflight and retry within policy or mark the candidate
stale. Conflicts, failing tests, ambiguity, or product judgment route to the
original worker, coordinator, or an integration worker. Failed temporary
checkouts may be retained briefly for diagnosis under a bounded cleanup
policy, but are not the human-facing Topic checkout.

---

# Integration skill for nontrivial cases

Provide a dedicated integration skill/instruction set for an agent when the
deterministic path cannot finish or policy asks for review.

The skill should tell the agent to:

1. inspect source and target state;
2. verify the source result is actually complete;
3. identify relevant commits/diff;
4. update/fetch target state;
5. apply the requested integration strategy;
6. resolve straightforward conflicts;
7. run required validation/tests;
8. update the target branch/resource;
9. record resulting commits/PRs/resources;
10. report a concise outcome.

The skill should not assume every repository uses the same workflow.

Repository/Topic configuration may provide:

```text
target branch
preferred integration strategy
required tests
direct push allowed?
PR required?
commit conventions
merge policy
```

---

# Nontrivial integrations

A selected strategy must not force naive mechanical execution.

Example:

User chooses:

```text
Squash into topic
```

but the target branch has diverged.

Correct behavior:

```text
Integrating Zed remote support…

The topic branch changed since this worker started.
Resolving conflicts in:
  src/resources.ts
  src/zed.ts

Running tests…
```

The LLM may perform whatever intermediate steps are required while preserving the requested end-state semantics.

If the conflict requires product judgment, escalate rather than inventing a decision.

---

# Integration worker

The coordinator does not need to personally perform every difficult integration.

For complicated cases, it may spawn an **integration worker**.

Example:

```text
worker A ─┐
          ├─> integration worker ─> topic branch
worker B ─┘
```

The integration worker receives:

- target Topic/resource;
- source branch(es)/worktree(s);
- selected integration strategy;
- repository integration policy;
- relevant design/docs/resources.

It returns:

```text
status
summary
resulting commits
resulting PR, if any
validation status
conflicts/decisions made
unresolved issues
```

The coordinator remains responsible for accepting/escalating the result.

---

# Integration target

Do not assume Thread ancestry equals git ancestry.

Default behavior:

> Integrate into the Topic's canonical ref for the relevant repository.

Recursive worker → parent worker integration is an explicit path when the
parent owns an intermediate accepted ref; it does not change the normal
Topic-level queue. If the Topic has no canonical ref for that repository,
Arachne must establish or select a target before claiming readiness.

The target should always be inspectable/overrideable.

Example:

```text
Integrate into:
  Arachne / arachne repo / arachne-dev
```

Advanced users should be able to choose another target if necessary.

---

# Topic branches

A Topic may define one canonical branch/reference per repository.

Example:

```text
Topic: Arachne

Resources

arachne
  topic branch: arachne-dev

marin-community/loom
  topic branch: arachne
```

A Topic therefore does not have one universal git branch.

It may span multiple repositories, each with its own accepted state.

Workers should normally fork from the current Topic branch for the relevant repository.

Important:

> Worker hierarchy and git branch ancestry must remain independent.

A child worker spawned for conceptual reasons should not automatically fork from its parent worker's branch unless explicitly requested.

Default:

```text
new coding worker
→ fork from current topic branch
```

Explicit alternative:

```text
branch from this worker
```

---

# Integration worktree

A Topic may have an attached **integration worktree** for a repository.

This is a concrete checkout of the Topic branch.

Example:

```text
Topic: Arachne

repo: arachne
topic branch: arachne-dev
integration worktree:
  runner: DGX
  path: /...
```

The integration worktree is a Resource, not the Topic itself.

It may be deleted and reconstructed.

The durable canonical object is the branch/ref.

Default policy:

> Ordinary workers should not directly write to the Topic integration worktree.

They receive isolated worktrees.

Integration is the operation that moves their work into accepted Topic state.

---

# Manual edits

The user may open either a worker worktree or the integration worktree in Zed and edit manually.

This is valid.

## Editing worker worktree

Manual changes become part of that worker's result.

The worker/thread should notice or at least accurately report the changed git state before integration.

## Editing integration worktree

Uncommitted manual changes are pending checkout state, not accepted Topic ref
state. Committing and advancing the ref changes canonical state. Before
integrating another worker, Arachne/Loom must detect checkout dirtiness and
refresh/reconcile the target ref rather than assuming the integration worktree
is unchanged. A temporary integration checkout prevents an attempted merge
from disturbing human edits.

---

# Land UI

At the Topic level, use a separate split button:

```text
[ Land ▼ ]
```

Land means:

> Move this Topic's accepted state into its upstream/external destination.

Possible strategies:

```text
Open PR
Squash into main
Merge into main
Rebase / fast-forward into main
Push topic branch
Ask coordinator to decide
```

Again, available options depend on repository policy.

Example:

```text
Topic: Arachne

topic branch: arachne-dev
upstream: main

[ Land ▼ ]
```

Primary click uses the remembered/default landing policy.

---

# Example repository policies

## Arachne repository

Possible configuration:

```text
worker integration:
  default: squash

topic branch:
  arachne-dev

landing:
  default: open PR
  target: main

required validation:
  cargo/test/etc.
```

Flow:

```text
worker branch
   ↓ squash
arachne-dev
   ↓ PR
main
```

## Loom repository

Possible configuration:

```text
worker integration:
  default: merge or cherry-pick

topic branch:
  arachne

landing:
  none by default
```

Flow:

```text
worker branch
   ↓ integrate
long-lived loom/arachne branch
```

Individual changes may later be upstreamed to Loom main intentionally.

---

# Thread UI

A completed coding worker should expose integration near the result.

Example:

```text
Zed remote support
Done

3 commits
+382 -91
Tests passing

Resources
  worktree: zed-remote
  branch: arachne/zed-remote

[Review Diff] [Open in Zed]

[ Integrate ▼ ]
```

After integration:

```text
Integrated into Arachne
squash · commit 7a29ef

[View commit]
```

The button should no longer imply outstanding work.

---

# Topic integration queue and coordinator UX

The Topic view should show queued candidates and outcomes across its workers,
including target revision, preflight status, and validation. The top-level
coordinator Thread also receives integration Events in its timeline. An Event
can update the queue without immediately waking the coordinator.

Example:

```text
[Worker]
Zed remote support completed.

3 commits · tests passing

[Integrate ▼] [Open Thread]
```

After action:

```text
[Integration]
Zed remote support integrated via squash.

Target:
  arachne-dev

Commit:
  7a29ef Add remote Zed checkout support

Validation:
  ✓ frontend tests
  ✓ cargo test
```

This should become part of durable Topic history.

---

# Agent-initiated integration

Eventually a coordinator may integrate workers without explicit user clicks.

This should be governed by Topic/repository policy.

Example:

```text
auto-integrate:
  allowed when:
    tests passing
    no conflicts
    worker marked complete
    integration strategy is configured
```

Anything outside policy should become coordinator or human attention.

Do not implement broad autonomous integration before the manual flow is trustworthy.

---

# Failure states

Integration should distinguish:

## Mechanical failure

Examples:

- git failure;
- tests fail;
- target unavailable.

Result:

```text
Integration failed
Tests failed in test_resource_binding
```

Coordinator may attempt repair.

## Conflict requiring engineering work

Spawn/continue integration reasoning.

Do not immediately bother user.

## Conflict requiring judgment

Escalate.

Example:

```text
Needs you

Both implementations change the resource ownership model differently.

Option A preserves…
Option B changes…
```

## Stale source

Worker branch has changed since review.

Refresh and re-evaluate before integrating.

---

# Resource updates

After successful integration:

- source worker remains historically linked;
- target Resource records resulting commit;
- PR association updates if appropriate;
- worker result is marked integrated;
- Topic canonical state advances;
- future workers should fork from the updated Topic state.

Do not delete the worker immediately.

Archival/cleanup is a separate lifecycle concern.

---

# Todo integration

Arachne should eventually maintain a durable cross-topic user-facing Todo
list, separate from each Topic's plan/backlog and from worker-internal todos.

Integration actions may generate/remove user todos.

Examples:

```text
□ Review attention UI worker
□ Decide whether to integrate Loom API change
□ Land Arachne topic
```

After an action completes:

```text
✓ Review attention UI worker
```

Do not conflate this with the worker's own implementation checklist.

---

# Phase 1 implementation scope

For first dogfooding, keep this narrow.

Required:

1. completed worker knows its source branch/worktree;
2. Topic knows its target branch per repo;
3. show `Integrate` split button;
4. support at least:
   - squash;
   - merge;
   - rebase;
   - cherry-pick;
   - open PR;
   - ask coordinator;
5. remember last-used strategy per repo;
6. button sends a structured integration request to Loom;
7. the coordinator skill handles nontrivial cases while clean cases can use a
   deterministic temporary-worktree path;
8. integration result appears in Topic queue and Thread history;
9. target git state updates;
10. future workers fork from updated Topic state.

Nice to have after the manual loop works:

- integration worker delegation;
- tests displayed inline;
- conflict escalation UI.
- target-revision readiness and automatic re-preflight of sleeping candidates.

Defer:

- autonomous integration;
- complex policy editor;
- visual git graph;
- broad autonomous merge queue;
- distributed locking beyond basic Resource lease/reconciliation.

---

# Dogfooding target

Use this immediately for Arachne development.

Expected workflow:

```text
Topic: Arachne
topic branch: arachne-dev

Workers:
  ✓ Tauri bootstrap
  ✓ Zed integration
  ● Attention UI
  ● SSE reconnect
```

User can:

1. open any worker Thread;
2. inspect its diff;
3. open its worktree in Zed;
4. make manual edits if desired;
5. click `Integrate`;
6. choose squash/merge/etc.;
7. let the coordinator perform and validate integration;
8. see the Topic branch advance;
9. launch subsequent workers from that updated Topic state;
10. eventually click `Land` to open/merge the Topic into main.

If this flow is smooth, Arachne should remove most of the branch/worktree bookkeeping currently required when developing with several agents in parallel.
