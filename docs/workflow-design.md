# Arachne Track Coordination Spec

## 1. Goal

Arachne Tracks are designed for **long-running, multi-threaded work where the top-level agent acts primarily as a coordinator/project manager rather than the main implementer**.

A Track coordinator should preserve its context for:

- understanding the user's goals;
- maintaining a coherent picture of the Track;
- decomposing work;
- deciding what should happen next;
- delegating;
- synthesizing worker results;
- deciding what requires review;
- integrating accepted work;
- escalating genuine judgment calls to the human.

It should avoid consuming its long-lived context with implementation details, experiment logs, large tool traces, or repetitive operational work when those can be delegated to shorter-lived Threads.

The point of delegation is therefore not merely parallelism.

> **Delegation is primarily a context-management strategy.**

A healthy Track may have many pieces of work happening concurrently.

---

# 2. Do not encode the workflow as a rigid state machine

Arachne should **not** require every worker to transition through a hard-coded sequence such as:

`working → review → approved → integrated → human review`

Reality is messier, and forcing every task through the same lifecycle will produce bureaucracy and wasted inference.

Instead:

- provide durable primitives;
- expose useful status/attention information;
- give agents strong behavioral guidance;
- let the coordinator bend the workflow when appropriate.

For example, a trivial typo does not need an independent reviewer. A large architectural change probably does.

The runtime should know facts like:

- Thread is active or sleeping;
- a worker has returned a result;
- a branch has unintegrated changes;
- validation passed or failed;
- a review exists;
- a PR changed;
- a human-attention request exists.

The **interpretation** of those facts belongs primarily to the coordinator policy.

---

# 3. Track coordinator role

The Track's primary Thread is its **coordinator**.

Its default posture is:

> **Act like a strong technical project manager who can understand engineering deeply, but prefers to delegate substantial execution.**

It is explicitly *not* merely a developer with permission to spawn subagents occasionally.

Its loop is approximately:

1. understand;
2. decompose;
3. delegate;
4. monitor through Events/results;
5. inspect/synthesize;
6. arrange review where useful;
7. integrate accepted work;
8. escalate only when necessary;
9. repeat.

Multiple work items may be active simultaneously.

The coordinator should be comfortable having:

- several implementation workers;
- a research worker;
- an experiment still running;
- one change under review;
- another change ready to integrate;
- a question awaiting the user;

all at the same time.

There is no single Track-wide "current task."

---

# 4. When the coordinator should delegate

The coordinator should **aggressively delegate non-trivial work**.

Strong reasons to delegate include work that:

- will involve substantial tool use;
- requires reading many files;
- will generate large logs/output;
- involves prolonged coding;
- requires experiments;
- requires broad research/scouting;
- can proceed independently from other active work;
- benefits from a clean bounded context;
- is likely to require several iterative attempts;
- would substantially enlarge the coordinator's context.

Typical delegated work:

- implementing a feature;
- debugging a nontrivial failure;
- running an experiment;
- investigating an unfamiliar subsystem;
- benchmarking;
- writing substantial tests;
- reviewing another worker's implementation;
- resolving a merge conflict;
- preparing a PR stack.

The coordinator **may still perform small technical actions itself**.

Examples:

- inspect a short diff;
- read a key file;
- run a quick query;
- check a configuration value;
- verify a worker's claim;
- make a tiny mechanical edit;
- gather enough information to delegate intelligently.

Do not instruct it simply:

> Never code.

That is too rigid and can make the coordinator ignorant of the work it is managing.

A better principle is:

> **Protect the coordinator's context. Do substantial execution elsewhere.**

---

# 5. Worker Threads

A delegated worker should receive a bounded assignment.

A good worker launch should communicate:

- objective;
- relevant Resources;
- constraints;
- expected deliverable;
- how much autonomy it has;
- what constitutes completion;
- when it should escalate.

Workers should generally own their task until it reaches a sensible stopping point.

For coding work, that often means:

- understand the issue;
- implement;
- test;
- clean up;
- commit/stabilize the change;
- summarize;
- make the work reviewable.

Workers should not return to the coordinator after every small uncertainty that they could reasonably resolve themselves.

Likewise, the coordinator should not micromanage workers turn-by-turn.

---

# 6. Recursive delegation

A worker may discover that its assignment contains a substantial independent subproblem.

It should be allowed to delegate further.

Thus:

```text
Track coordinator
    ├── Worker A
    │     ├── Worker A1
    │     └── Worker A2
    └── Worker B
```

A worker acting as a local coordinator should follow the same general principle:

> Preserve your own useful context; delegate expensive independent work.

Do not route every subtask through the top-level coordinator. That creates a bottleneck and unnecessarily fills the Track context.

---

# 7. Worker handoff

Workers should return a **concise durable handoff**, not require the parent to reread their entire transcript.

A useful handoff contains roughly:

- outcome/status;
- concise summary of what was learned/done;
- Resources created or changed;
- commits/branch/PR where relevant;
- validation performed and results;
- important decisions;
- unresolved issues;
- recommended next step.

Full Thread history remains available for drill-down.

The coordinator should normally consume the handoff first and inspect the worker history only when needed.

This is important for controlling context growth.

---

# 8. Review workflow

For substantive work, independent review should usually happen **before human review**.

Default coding flow:

```text
coordinator
    ↓ delegates
implementation worker
    ↓ thinks work is ready
review worker
    ↓
implementation worker fixes issues if needed
    ↓
review passes
    ↓
result returns to coordinator
```

The reviewer should normally be a **fresh Thread** with enough context to understand the task and inspect the result, but not the implementation worker's full reasoning history.

That provides some independence.

The reviewer may:

- inspect code/diff;
- run tests;
- identify missing cases;
- check against the original objective;
- flag design concerns;
- approve the result.

If review finds actionable issues, those should normally go back to the original worker.

This loop can repeat without involving the Track coordinator on every iteration.

---

# 9. Review is risk-based, not mandatory

Do not require independent review for every delegated action.

Review is strongly encouraged when:

- code is substantial;
- behavior is difficult to validate;
- changes affect architecture;
- failures could be expensive;
- integration touches shared/high-risk areas;
- experimental conclusions will guide expensive follow-up work;
- the coordinator lacks confidence.

Review may be skipped for:

- trivial mechanical edits;
- obviously safe cleanup;
- narrow changes with strong deterministic tests;
- disposable exploratory work;
- cases where the coordinator has directly inspected enough evidence.

This should be coordinator judgment or Track policy, not an inflexible runtime rule.

---

# 10. Explicit Changes land into accepted Track state

Threads produce ZERO, ONE or MULTIPLE explicit Changes (bounded branches/diffs).
The Changes tab contains the queue of Changes, each with a stable source-qualified ID,
title, original repository/branch/worktree, immutable scope, authenticated
intended-completion reference and per-output receipt. See
[Change v1 contract](thread-landing-contract.md). Status, title, completion and
merely having a Loom branch create no output or Land obligation. Successful
review/research/coordination tasks commonly report `changes:[]`; even an unmarked
reviewer then has no Land action. A Thread may produce a later new Change after
an earlier output was accepted. Legacy `integration_result` cannot hide it or
mark all siblings accepted. Repeated historical completion produces no new delta.

Select ONE declared Change to review or Land. A working bounded output may
request noninterrupting wrap-up behind its active ACP turn. One click includes
review/repair/commit/validation/acceptance without another Prepare click.
Terminal compatibility uses durable channel delivery. Default squash accepts
only that delta into the canonical Track ref; optional merge/rebase/cherry-pick
must exclude siblings and accepted history. Preserve foreign staged, unstaged,
untracked and ignored files; reconcile in isolated worktrees.

New Tracks initialize real editable `arachne/tracks/<slug>` topic refs from
the launch seed, persisted as `track_topic_branch`, independently of Loom scratch.
Legacy no-tag Tracks retain their accepted branch. Immediately before EVERY
acceptance write reverify live same-repository owning Track, initialization,
canonical ref/revision, original source identity and selected scope/completion/
receipt. Changed selections and finalized/ repaired scopes require fresh review.
A queued Land is not success; only actual acceptance/validation stamps the
selected `change_acceptance:<id>` receipt. Review → Prepare is optional selected
preparation only, never acceptance or publication.

Autoland defaults Off, with source Inherit/On/Off overrides in Loom. It accepts
eligible explicit outputs in successful intended own-channel final completions,
never checkpoint/help/partial/interrupted/review-only results. Live policy and
identity guards suppress stale queued actions; authenticated user interruptions
never authorize automatic resumption. Nested sources need real verified result
wake delivery before waiting. No scheduler, polling, implicit PR or main write.

---

# 11. Ready is maintained, not historical

A worker should not become "ready" merely because it stopped producing output.

For coding work, workers should be encouraged to reach a condition resembling:

- changes stabilized/committed;
- required validation passed;
- concise handoff ready;
- mergeability preflight clean against current Track target.

Conceptually:

`Ready against Track@abc123`

Then the worker can sleep.

If another integration advances the Track:

`abc123 → def456`

the control plane can deterministically re-run mergeability checks for sleeping candidates.

If still clean:

- do nothing;
- leave worker asleep.

If newly conflicting:

- emit an Event;
- wake the original worker to reconcile;
- revalidate;
- return to Ready.

Thus worker execution status and integration status should remain conceptually separate.

---

# 12. Human review

The user should generally see work **after agent review and Track integration**, not be asked to supervise every worker.

A desirable flow:

```text
several workers
     ↓
agent review/fixes
     ↓
integrated into Track feature branch
     ↓
coordinator says:
"Here is the coherent change. Please review."
     ↓
human reviews actual accepted state
```

This minimizes human context switching.

The human may:

- inspect diff;
- open the Track checkout in Zed;
- chat with coordinator;
- request changes;
- approve.

Human approval does not necessarily mean immediate merge to main.

---

# 13. Track Publish

Change **Land** accepts one selected output into the canonical Track topic branch.
Track **Publish** moves only that accepted state to an external destination
under the user's explicit chosen action. Autoland never supplies publishing
authority; coordinator scratch work is not accepted publishing source.

Choices include Single PR, a PR stack, or chosen local/upstream
squash/merge/rebase/push. A stack defines ordered accepted changes, head refs
and dependent bases, validates each layer, and creates/reports actual
separate PR links; a single PR is not a stack. Local publishing uses the
primary checkout's CURRENT branch with no assumed main/recorded-base
fallback, preserving foreign staged/unstaged/untracked target work. Clean
suitable local publication remains synchronous; recoverable staged-target
or safely unwound conflict repair queues a complete chosen publication
request, never a premature success state.

`publish_result` records actual successful publication separately from
worker acceptance. User review of coherent accepted state may precede Publish;
partial results or stopping alone are not readiness for either operation.

---

# 14. Attention

Human Attention is scarce.

The system should aggressively distinguish:

- worker needs help;
- parent needs to act;
- reviewer found issues;
- integration conflict;
- system Event occurred;

from:

- **human needs to decide something**.

A worker should normally escalate first to its parent.

The parent may:

- answer directly;
- supply context;
- spawn another worker;
- switch to a stronger model;
- grant/request a Resource;
- perform a small inspection.

Only when the hierarchy cannot reasonably resolve the issue should it become **Needs You**.

`attention` and `blocked` explicitly request human action. They must not mean
"my parent needs to review this" or "another agent must act". A delegated
reviewer's approval, requested changes, failed checks, request for context or
"ready for review" handoff belongs to its immediate parent. The child keeps
status `ok` and publishes a typed result on its own Loom channel, which wakes
the parent. A concrete help request may also use `parent_help=requested`.
Intermediate parents resolve or forward the request through the hierarchy.

For example, **Review child interruption parent wake** sends findings or its
verdict to the interruption worker. That worker owns repairs and escalates to
the Track coordinator if needed; the user does not need to supervise that
review. Repository instructions to request final review use this parent
handoff for delegated work. Human review is requested only for a concrete
prepared result that requires the user's judgment or a specific decision or
access requirement the hierarchy cannot resolve.

If a child mistakenly raises human attention for agent-owned work, its parent
takes ownership of the handoff and has the child clear that status. The
mistaken escalation does not transfer coordination responsibility to the user.

Any human escalation should ideally include:

- what happened;
- why the agent cannot resolve it;
- what decision/action is needed;
- recommended option;
- relevant alternatives.

Avoid vague:

> What should I do?

Prefer:

> The two implementations encode incompatible checkpoint semantics. I recommend A because X. B preserves compatibility but costs Y. Which do you want?

---

# 15. Track durable state

Do not rely exclusively on the coordinator conversation history for long-term Track state.

A Track should maintain a compact durable brief/state containing things like:

- current objective;
- important decisions;
- constraints;
- active work;
- significant Resources;
- unresolved questions;
- current integration/landing state;
- notable experimental conclusions.

The coordinator should update/curate this as work progresses.

Worker handoffs may propose updates, but the coordinator owns the coherent Track-level version.

This lets the coordinator's conversational context compact or restart without losing the project.

---

# 16. Drill-down

Summaries are intentionally lossy.

Therefore the coordinator must always be able to intentionally inspect:

- worker Thread history;
- diff;
- commits;
- artifacts;
- experiment results;
- review output;
- associated Resources.

The goal is not to forbid detailed context. It is to **load it on demand rather than carry it forever**.

---

# 17. Events

**Event** is the common mechanism for external and internal activity.

Examples:

- worker completed;
- worker requested help;
- review finished;
- Topic/Track ref advanced;
- PR review arrived;
- CI changed;
- experiment completed;
- W&B alert fired;
- timer fired;
- webhook arrived;
- another Thread sent information;
- runner came online.

Events enter a durable mailbox.

An Event does **not** imply that a full Track coordinator should wake.

---

# 18. Automation triage

Automations should optimize heavily for **avoiding expensive context prefill**.

Do not repeatedly wake a large high-quality coordinator with its full Track context just because a cron timer or webhook fired.

Default automation architecture:

```text
Event arrives
     ↓
tiny ephemeral triage Thread
     ↓
small/cheap decision model
     ↓
Does anything need to happen?
     ├── no → record/ignore/coalesce
     └── yes
           ↓
      choose appropriate action
```

The triage Thread should receive only:

- Event payload;
- minimal source metadata;
- small relevant policy;
- perhaps a compact Track summary if required.

It should generally **not inherit the full coordinator transcript**.

Suitable triage models might be:

- a lightweight general model;
- a dedicated decision model;
- Luna-class inexpensive inference;
- deterministic rules where possible.

Typical output:

```text
action_required: false
reason: CI passed; no pending dependency
```

or:

```text
action_required: true
target: worker-thread-123
action: wake
reason: PR reviewer requested code changes
```

---

# 19. Wake routing

If automation triage decides action is required, it should wake the **smallest appropriate context**.

Examples:

PR gets simple review comment:

```text
Event
→ triage
→ wake original worker
```

Experiment finishes:

```text
Event
→ triage
→ wake experiment/research worker
```

Several experimental results together require strategic interpretation:

```text
Events
→ coalesce
→ wake Track coordinator
```

Ambiguous product decision:

```text
worker
→ coordinator
→ Needs You
```

Do not default to waking the top-level Track.

---

# 20. Event coalescing

Avoid spending inference on rapid bursts of related Events.

For example:

```text
PR synchronize
CI pending
CI job 1 passed
CI job 2 passed
CI complete
```

may be coalesced into one useful wake:

> PR HEAD changed; CI now passes.

Likewise several W&B updates can wait until a meaningful condition changes.

Subscriptions should support debounce/coalescing semantics where useful.

---

# 21. Heartbeats and cron

Heartbeat, cron, and webhook receipt should all be understood as Event sources.

A heartbeat is not special orchestration machinery.

For example:

```text
timer fires
→ heartbeat Event
→ cheap triage
→ nothing appears stuck
→ no further action
```

If the triager detects that something genuinely appears abandoned:

```text
→ wake responsible Thread
```

This avoids spending expensive Track-context inference merely to establish that nothing happened.

---

# 22. Cross-Thread messaging

`send_to_thread` should be a first-class agent tool, with a matching UI action.

A Thread can send:

- note;
- concise finding;
- selected Resource;
- artifact;
- generated summary.

Conceptually:

```text
send_to_thread(
    target,
    message,
    resources=[]
)
```

The recipient receives a durable Event/message with provenance.

Delivery does not necessarily imply immediate wake; normal Event/wake policy applies.

This provides practical inter-agent coordination without building a free-form multi-agent chatroom.

---

# 23. Model escalation

Workers should be able to conclude:

> This task is outside my capability.

That should not immediately mean human escalation.

They can request:

- stronger model;
- different inference route;
- more context;
- another specialist worker.

For example:

```text
cheap GLM worker
    ↓ can't resolve
parent
    ↓
high-effort Codex worker
```

Human sees nothing unless the hierarchy ultimately needs judgment.

---

# 24. UI implications

The UI should not expose a giant literal workflow state machine.

Instead it should communicate useful facts.

For a Track:

**Needs You**
- genuinely human-blocked work.

**Working**
- active coordinator/workers.

**Ready / Integration**
- candidates ready to integrate;
- candidates stale/conflicting;
- integration progress.

**Waiting**
- PR review;
- experiment;
- CI;
- external dependency.

**Resting**
- dormant Threads with no pending action.

Inside a Track, the Threads panel should make the portfolio visible, but not look like Jira.

Example:

```text
Threads

● implement resource bindings
  working

◐ review resource bindings
  reviewing

✓ Zed remote support
  ready to integrate

○ experiment 32x4
  waiting on W&B

○ old investigation
  resting
```

These labels can be derived/advisory rather than a rigid persisted workflow enum.

---

# 25. Coordinator instructions

The coordinator's durable instructions should strongly communicate something like:

> You coordinate this Track. Your highest-value context is the user's goals, the overall plan, important decisions, worker results, and the current state of the project.
>
> Delegate substantial execution aggressively. Implementation, lengthy debugging, experimentation, research scouting, and other context-heavy work should normally occur in child Threads.
>
> You are technically capable and may inspect code, run small checks, or make trivial changes when doing so helps you manage the work. Do not delegate ceremony for its own sake.
>
> Multiple independent pieces of work may proceed simultaneously.
>
> When a worker believes substantial work is complete, usually arrange independent review before accepting it. Route review feedback back to the original worker where practical.
>
> Integrate accepted worker results into the Track's canonical Resources. Escalate to the user only for decisions or review that genuinely require human judgment.
>
> Protect your context. Prefer concise worker handoffs and durable Track state over ingesting entire worker histories. Drill down when needed.
>
> Do not busy-wait. Use Events/subscriptions and allow yourself and your workers to sleep while external work is pending.

Exact wording can evolve through dogfooding.

---

# 26. Worker instructions

Default worker guidance should say approximately:

> Own the delegated objective. Work independently and use your tools freely within your Resources.
>
> Resolve ordinary implementation questions yourself. Escalate when you need parent context, permissions, a stronger model, or a genuine decision.
>
> Before declaring substantive coding work ready, stabilize the changes, run appropriate validation, summarize the result, and make the work reviewable.
>
> You may delegate substantial independent subproblems to child workers when that helps preserve your context or parallelize work.
>
> Do not poll external systems unnecessarily. Register/wait for Events where supported.

---

# 27. Reviewer instructions

Reviewers should be instructed to act independently:

> Review the candidate against its stated objective and the target codebase. Inspect the diff and relevant surrounding code, run useful validation, and look for correctness, regressions, missing tests, integration problems, and unnecessary complexity.
>
> Do not merely restate what the implementation worker claims.
>
> If there are actionable issues, return a concise review suitable for the implementation worker. If the work is sound, explicitly say it is ready for integration.

---

# 28. Anti-patterns

We should regard these as signs the coordination design is failing:

### Coordinator does all implementation itself

Delegation exists only cosmetically.

### Coordinator delegates every five-minute task

Agent bureaucracy overwhelms useful work.

### Every worker completion triggers human Attention

Hierarchy is not doing its job.

### Every Event wakes the expensive coordinator

Automation architecture is wasting prefill/context.

### Workers stay alive polling

Events/subscriptions are not being used.

### Reviewer always agrees with worker

Review is ceremonial or over-contextualized.

### Coordinator must reread every full transcript

Worker handoffs/Track state are inadequate.

### Worker hierarchy is forced to mirror git branch ancestry

Coordination model is leaking into version-control topology.

### UI requires users to understand every internal state transition

We have accidentally built Jira for agents.

---

# 29. Initial implementation priorities

Do not implement all of this as infrastructure at once.

First:

1. strengthen coordinator/worker/reviewer prompts;
2. expose reliable parent/child relationships;
3. give coordinator straightforward spawn/review/send-to-thread tools;
4. normalize concise worker result/handoff;
5. make review spawning easy;
6. make declared Change Land available in the Track Changes queue;
7. make Attention require explicit human escalation;
8. improve UI visibility into active/ready/waiting/resting Threads.

Then:

9. add automation triage Threads;
10. support lightweight/decision-model routing;
11. add Event coalescing;
12. add sleeping-worker wake behavior;
13. improve maintained mergeability/Ready checks;
14. refine durable Track state.

Dogfood before hardening any of these into strict orchestration rules.

---

# 30. Guiding principle

The runtime should be permissive; the agents should be opinionated.

> **Encode the preferred coordination model in prompts, tools, skills, and UI affordances—not in a rigid workflow engine.**

The happy path is:

```text
human defines direction
        ↓
Track coordinator decomposes
        ↓
workers execute
        ↓
agents review
        ↓
Track accepts selected Change into topic ref
        ↓
human reviews coherent Track state
        ↓
explicit Track Publish
```

But the system must remain flexible enough to skip, recurse, retry, inspect, or restructure that flow when the work demands it.

And for background activity:

```text
Event
  ↓
cheap context-free-ish triage
  ↓
ignore / deterministic action / wake smallest useful Thread
  ↓
wake expensive coordinator only when necessary
```

That second loop is as important as the first: it is what lets a long-running Track remain intelligent without repeatedly paying to prefill a huge high-quality model context just to discover that nothing needs doing.Yes. I’d make this a **behavioral/orchestration spec**, explicitly not a workflow-engine spec. The runtime should expose the primitives; the agent instructions should teach the preferred loop.

# Arachne Track Coordination Spec

## 1. Goal

Arachne Tracks are designed for **long-running, multi-threaded work where the top-level agent acts primarily as a coordinator/project manager rather than the main implementer**.

A Track coordinator should preserve its context for:

- understanding the user's goals;
- maintaining a coherent picture of the Track;
- decomposing work;
- deciding what should happen next;
- delegating;
- synthesizing worker results;
- deciding what requires review;
- integrating accepted work;
- escalating genuine judgment calls to the human.

It should avoid consuming its long-lived context with implementation details, experiment logs, large tool traces, or repetitive operational work when those can be delegated to shorter-lived Threads.

The point of delegation is therefore not merely parallelism.

> **Delegation is primarily a context-management strategy.**

A healthy Track may have many pieces of work happening concurrently.

---

# 2. Do not encode the workflow as a rigid state machine

Arachne should **not** require every worker to transition through a hard-coded sequence such as:

`working → review → approved → integrated → human review`

Reality is messier, and forcing every task through the same lifecycle will produce bureaucracy and wasted inference.

Instead:

- provide durable primitives;
- expose useful status/attention information;
- give agents strong behavioral guidance;
- let the coordinator bend the workflow when appropriate.

For example, a trivial typo does not need an independent reviewer. A large architectural change probably does.

The runtime should know facts like:

- Thread is active or sleeping;
- a worker has returned a result;
- a branch has unintegrated changes;
- validation passed or failed;
- a review exists;
- a PR changed;
- a human-attention request exists.

The **interpretation** of those facts belongs primarily to the coordinator policy.

---

# 3. Track coordinator role

The Track's primary Thread is its **coordinator**.

Its default posture is:

> **Act like a strong technical project manager who can understand engineering deeply, but prefers to delegate substantial execution.**

It is explicitly *not* merely a developer with permission to spawn subagents occasionally.

Its loop is approximately:

1. understand;
2. decompose;
3. delegate;
4. monitor through Events/results;
5. inspect/synthesize;
6. arrange review where useful;
7. integrate accepted work;
8. escalate only when necessary;
9. repeat.

Multiple work items may be active simultaneously.

The coordinator should be comfortable having:

- several implementation workers;
- a research worker;
- an experiment still running;
- one change under review;
- another change ready to integrate;
- a question awaiting the user;

all at the same time.

There is no single Track-wide "current task."

---

# 4. When the coordinator should delegate

The coordinator should **aggressively delegate non-trivial work**.

Strong reasons to delegate include work that:

- will involve substantial tool use;
- requires reading many files;
- will generate large logs/output;
- involves prolonged coding;
- requires experiments;
- requires broad research/scouting;
- can proceed independently from other active work;
- benefits from a clean bounded context;
- is likely to require several iterative attempts;
- would substantially enlarge the coordinator's context.

Typical delegated work:

- implementing a feature;
- debugging a nontrivial failure;
- running an experiment;
- investigating an unfamiliar subsystem;
- benchmarking;
- writing substantial tests;
- reviewing another worker's implementation;
- resolving a merge conflict;
- preparing a PR stack.

The coordinator **may still perform small technical actions itself**.

Examples:

- inspect a short diff;
- read a key file;
- run a quick query;
- check a configuration value;
- verify a worker's claim;
- make a tiny mechanical edit;
- gather enough information to delegate intelligently.

Do not instruct it simply:

> Never code.

That is too rigid and can make the coordinator ignorant of the work it is managing.

A better principle is:

> **Protect the coordinator's context. Do substantial execution elsewhere.**

---

# 5. Worker Threads

A delegated worker should receive a bounded assignment.

A good worker launch should communicate:

- objective;
- relevant Resources;
- constraints;
- expected deliverable;
- how much autonomy it has;
- what constitutes completion;
- when it should escalate.

Workers should generally own their task until it reaches a sensible stopping point.

For coding work, that often means:

- understand the issue;
- implement;
- test;
- clean up;
- commit/stabilize the change;
- summarize;
- make the work reviewable.

Workers should not return to the coordinator after every small uncertainty that they could reasonably resolve themselves.

Likewise, the coordinator should not micromanage workers turn-by-turn.

---

# 6. Recursive delegation

A worker may discover that its assignment contains a substantial independent subproblem.

It should be allowed to delegate further.

Thus:

```text
Track coordinator
    ├── Worker A
    │     ├── Worker A1
    │     └── Worker A2
    └── Worker B
```

A worker acting as a local coordinator should follow the same general principle:

> Preserve your own useful context; delegate expensive independent work.

Do not route every subtask through the top-level coordinator. That creates a bottleneck and unnecessarily fills the Track context.

---

# 7. Worker handoff

Workers should return a **concise durable handoff**, not require the parent to reread their entire transcript.

A useful handoff contains roughly:

- outcome/status;
- concise summary of what was learned/done;
- Resources created or changed;
- commits/branch/PR where relevant;
- validation performed and results;
- important decisions;
- unresolved issues;
- recommended next step.

Full Thread history remains available for drill-down.

The coordinator should normally consume the handoff first and inspect the worker history only when needed.

This is important for controlling context growth.

---

# 8. Review workflow

For substantive work, independent review should usually happen **before human review**.

Default coding flow:

```text
coordinator
    ↓ delegates
implementation worker
    ↓ thinks work is ready
review worker
    ↓
implementation worker fixes issues if needed
    ↓
review passes
    ↓
result returns to coordinator
```

The reviewer should normally be a **fresh Thread** with enough context to understand the task and inspect the result, but not the implementation worker's full reasoning history.

That provides some independence.

The reviewer may:

- inspect code/diff;
- run tests;
- identify missing cases;
- check against the original objective;
- flag design concerns;
- approve the result.

If review finds actionable issues, those should normally go back to the original worker.

This loop can repeat without involving the Track coordinator on every iteration.

---

# 9. Review is risk-based, not mandatory

Do not require independent review for every delegated action.

Review is strongly encouraged when:

- code is substantial;
- behavior is difficult to validate;
- changes affect architecture;
- failures could be expensive;
- integration touches shared/high-risk areas;
- experimental conclusions will guide expensive follow-up work;
- the coordinator lacks confidence.

Review may be skipped for:

- trivial mechanical edits;
- obviously safe cleanup;
- narrow changes with strong deterministic tests;
- disposable exploratory work;
- cases where the coordinator has directly inspected enough evidence.

This should be coordinator judgment or Track policy, not an inflexible runtime rule.

---

# 10. Integration

Worker completion and worker review do **not** automatically mutate the Track's accepted state.

Substantive coding work becomes an **integration candidate**.

The Track owns integration.

Typical path:

```text
implementation
    ↓
review
    ↓
candidate ready
    ↓
integrate into Track
    ↓
Track canonical/topic ref advances
```

Integration may use:

- squash;
- merge;
- rebase;
- cherry-pick;
- PR into the Track branch;
- coordinator-selected strategy.

A clean mechanical integration should not require an LLM.

Prefer a deterministic path using a disposable integration worktree:

1. prepare target at current Track ref;
2. apply candidate with chosen strategy;
3. run configured validation;
4. advance the target ref;
5. clean up.

Use an LLM/worker when:

- conflicts arise;
- tests fail;
- integration requires interpretation;
- several candidates must be reconciled;
- strategy is ambiguous.

The original worker is often the best first agent to wake for a conflict involving its own work.

---

# 11. Ready is maintained, not historical

A worker should not become "ready" merely because it stopped producing output.

For coding work, workers should be encouraged to reach a condition resembling:

- changes stabilized/committed;
- required validation passed;
- concise handoff ready;
- mergeability preflight clean against current Track target.

Conceptually:

`Ready against Track@abc123`

Then the worker can sleep.

If another integration advances the Track:

`abc123 → def456`

the control plane can deterministically re-run mergeability checks for sleeping candidates.

If still clean:

- do nothing;
- leave worker asleep.

If newly conflicting:

- emit an Event;
- wake the original worker to reconcile;
- revalidate;
- return to Ready.

Thus worker execution status and integration status should remain conceptually separate.

---

# 12. Human review

The user should generally see work **after agent review and Track integration**, not be asked to supervise every worker.

A desirable flow:

```text
several workers
     ↓
agent review/fixes
     ↓
integrated into Track feature branch
     ↓
coordinator says:
"Here is the coherent change. Please review."
     ↓
human reviews actual accepted state
```

This minimizes human context switching.

The human may:

- inspect diff;
- open the Track checkout in Zed;
- chat with coordinator;
- request changes;
- approve.

Human approval does not necessarily mean immediate merge to main.

---

# 13. Land

**Land** is distinct from Integrate.

- **Integrate** moves child work into the Track's accepted state.
- **Land** moves the Track's accepted state to its external destination.

Examples:

```text
workers
   ↓ integrate
feature/topic branch
   ↓ land
main
```

or:

```text
workers
   ↓ integrate
feature/topic branch
   ↓ land
GitHub PR → main
```

Land behavior is determined by Project/Track policy.

Possible policies:

- open PR;
- squash into main;
- merge;
- rebase/fast-forward;
- push long-lived feature branch;
- custom landing skill.

The user may explicitly press **Land**, or potentially authorize autonomous landing under a suitable policy later.

---

# 14. Attention

Human Attention is scarce.

The system should aggressively distinguish:

- worker needs help;
- parent needs to act;
- reviewer found issues;
- integration conflict;
- system Event occurred;

from:

- **human needs to decide something**.

A worker should normally escalate first to its parent.

The parent may:

- answer directly;
- supply context;
- spawn another worker;
- switch to a stronger model;
- grant/request a Resource;
- perform a small inspection.

Only when the hierarchy cannot reasonably resolve the issue should it become **Needs You**.

Any human escalation should ideally include:

- what happened;
- why the agent cannot resolve it;
- what decision/action is needed;
- recommended option;
- relevant alternatives.

Avoid vague:

> What should I do?

Prefer:

> The two implementations encode incompatible checkpoint semantics. I recommend A because X. B preserves compatibility but costs Y. Which do you want?

---

# 15. Track durable state

Do not rely exclusively on the coordinator conversation history for long-term Track state.

A Track should maintain a compact durable brief/state containing things like:

- current objective;
- important decisions;
- constraints;
- active work;
- significant Resources;
- unresolved questions;
- current integration/landing state;
- notable experimental conclusions.

The coordinator should update/curate this as work progresses.

Worker handoffs may propose updates, but the coordinator owns the coherent Track-level version.

This lets the coordinator's conversational context compact or restart without losing the project.

---

# 16. Drill-down

Summaries are intentionally lossy.

Therefore the coordinator must always be able to intentionally inspect:

- worker Thread history;
- diff;
- commits;
- artifacts;
- experiment results;
- review output;
- associated Resources.

The goal is not to forbid detailed context. It is to **load it on demand rather than carry it forever**.

---

# 17. Events

**Event** is the common mechanism for external and internal activity.

Examples:

- worker completed;
- worker requested help;
- review finished;
- Topic/Track ref advanced;
- PR review arrived;
- CI changed;
- experiment completed;
- W&B alert fired;
- timer fired;
- webhook arrived;
- another Thread sent information;
- runner came online.

Events enter a durable mailbox.

An Event does **not** imply that a full Track coordinator should wake.

---

# 18. Automation triage

Automations should optimize heavily for **avoiding expensive context prefill**.

Do not repeatedly wake a large high-quality coordinator with its full Track context just because a cron timer or webhook fired.

Default automation architecture:

```text
Event arrives
     ↓
tiny ephemeral triage Thread
     ↓
small/cheap decision model
     ↓
Does anything need to happen?
     ├── no → record/ignore/coalesce
     └── yes
           ↓
      choose appropriate action
```

The triage Thread should receive only:

- Event payload;
- minimal source metadata;
- small relevant policy;
- perhaps a compact Track summary if required.

It should generally **not inherit the full coordinator transcript**.

Suitable triage models might be:

- a lightweight general model;
- a dedicated decision model;
- Luna-class inexpensive inference;
- deterministic rules where possible.

Typical output:

```text
action_required: false
reason: CI passed; no pending dependency
```

or:

```text
action_required: true
target: worker-thread-123
action: wake
reason: PR reviewer requested code changes
```

---

# 19. Wake routing

If automation triage decides action is required, it should wake the **smallest appropriate context**.

Examples:

PR gets simple review comment:

```text
Event
→ triage
→ wake original worker
```

Experiment finishes:

```text
Event
→ triage
→ wake experiment/research worker
```

Several experimental results together require strategic interpretation:

```text
Events
→ coalesce
→ wake Track coordinator
```

Ambiguous product decision:

```text
worker
→ coordinator
→ Needs You
```

Do not default to waking the top-level Track.

---

# 20. Event coalescing

Avoid spending inference on rapid bursts of related Events.

For example:

```text
PR synchronize
CI pending
CI job 1 passed
CI job 2 passed
CI complete
```

may be coalesced into one useful wake:

> PR HEAD changed; CI now passes.

Likewise several W&B updates can wait until a meaningful condition changes.

Subscriptions should support debounce/coalescing semantics where useful.

---

# 21. Heartbeats and cron

Heartbeat, cron, and webhook receipt should all be understood as Event sources.

A heartbeat is not special orchestration machinery.

For example:

```text
timer fires
→ heartbeat Event
→ cheap triage
→ nothing appears stuck
→ no further action
```

If the triager detects that something genuinely appears abandoned:

```text
→ wake responsible Thread
```

This avoids spending expensive Track-context inference merely to establish that nothing happened.

---

# 22. Cross-Thread messaging

`send_to_thread` should be a first-class agent tool, with a matching UI action.

A Thread can send:

- note;
- concise finding;
- selected Resource;
- artifact;
- generated summary.

Conceptually:

```text
send_to_thread(
    target,
    message,
    resources=[]
)
```

The recipient receives a durable Event/message with provenance.

Delivery does not necessarily imply immediate wake; normal Event/wake policy applies.

This provides practical inter-agent coordination without building a free-form multi-agent chatroom.

---

# 23. Model escalation

Workers should be able to conclude:

> This task is outside my capability.

That should not immediately mean human escalation.

They can request:

- stronger model;
- different inference route;
- more context;
- another specialist worker.

For example:

```text
cheap GLM worker
    ↓ can't resolve
parent
    ↓
high-effort Codex worker
```

Human sees nothing unless the hierarchy ultimately needs judgment.

---

# 24. UI implications

The UI should not expose a giant literal workflow state machine.

Instead it should communicate useful facts.

For a Track:

**Needs You**
- genuinely human-blocked work.

**Working**
- active coordinator/workers.

**Ready / Integration**
- candidates ready to integrate;
- candidates stale/conflicting;
- integration progress.

**Waiting**
- PR review;
- experiment;
- CI;
- external dependency.

**Resting**
- dormant Threads with no pending action.

Inside a Track, the Threads panel should make the portfolio visible, but not look like Jira.

Example:

```text
Threads

● implement resource bindings
  working

◐ review resource bindings
  reviewing

✓ Zed remote support
  ready to integrate

○ experiment 32x4
  waiting on W&B

○ old investigation
  resting
```

These labels can be derived/advisory rather than a rigid persisted workflow enum.

---

# 25. Coordinator instructions

The coordinator's durable instructions should strongly communicate something like:

> You coordinate this Track. Your highest-value context is the user's goals, the overall plan, important decisions, worker results, and the current state of the project.
>
> Delegate substantial execution aggressively. Implementation, lengthy debugging, experimentation, research scouting, and other context-heavy work should normally occur in child Threads.
>
> You are technically capable and may inspect code, run small checks, or make trivial changes when doing so helps you manage the work. Do not delegate ceremony for its own sake.
>
> Multiple independent pieces of work may proceed simultaneously.
>
> When a worker believes substantial work is complete, usually arrange independent review before accepting it. Route review feedback back to the original worker where practical.
>
> Integrate accepted worker results into the Track's canonical Resources. Escalate to the user only for decisions or review that genuinely require human judgment.
>
> Protect your context. Prefer concise worker handoffs and durable Track state over ingesting entire worker histories. Drill down when needed.
>
> Do not busy-wait. Use Events/subscriptions and allow yourself and your workers to sleep while external work is pending.

Exact wording can evolve through dogfooding.

---

# 26. Worker instructions

Default worker guidance should say approximately:

> Own the delegated objective. Work independently and use your tools freely within your Resources.
>
> Resolve ordinary implementation questions yourself. Escalate when you need parent context, permissions, a stronger model, or a genuine decision.
>
> Before declaring substantive coding work ready, stabilize the changes, run appropriate validation, summarize the result, and make the work reviewable.
>
> You may delegate substantial independent subproblems to child workers when that helps preserve your context or parallelize work.
>
> Do not poll external systems unnecessarily. Register/wait for Events where supported.

---

# 27. Reviewer instructions

Reviewers should be instructed to act independently:

> Review the candidate against its stated objective and the target codebase. Inspect the diff and relevant surrounding code, run useful validation, and look for correctness, regressions, missing tests, integration problems, and unnecessary complexity.
>
> Do not merely restate what the implementation worker claims.
>
> If there are actionable issues, return a concise review suitable for the implementation worker. If the work is sound, explicitly say it is ready for integration.

---

# 28. Anti-patterns

We should regard these as signs the coordination design is failing:

### Coordinator does all implementation itself

Delegation exists only cosmetically.

### Coordinator delegates every five-minute task

Agent bureaucracy overwhelms useful work.

### Every worker completion triggers human Attention

Hierarchy is not doing its job.

### Every Event wakes the expensive coordinator

Automation architecture is wasting prefill/context.

### Workers stay alive polling

Events/subscriptions are not being used.

### Reviewer always agrees with worker

Review is ceremonial or over-contextualized.

### Coordinator must reread every full transcript

Worker handoffs/Track state are inadequate.

### Worker hierarchy is forced to mirror git branch ancestry

Coordination model is leaking into version-control topology.

### UI requires users to understand every internal state transition

We have accidentally built Jira for agents.

---

# 29. Initial implementation priorities

Do not implement all of this as infrastructure at once.

First:

1. strengthen coordinator/worker/reviewer prompts;
2. expose reliable parent/child relationships;
3. give coordinator straightforward spawn/review/send-to-thread tools;
4. normalize concise worker result/handoff;
5. make review spawning easy;
6. make Integrate available at Track level;
7. make Attention require explicit human escalation;
8. improve UI visibility into active/ready/waiting/resting Threads.

Then:

9. add automation triage Threads;
10. support lightweight/decision-model routing;
11. add Event coalescing;
12. add sleeping-worker wake behavior;
13. improve maintained mergeability/Ready checks;
14. refine durable Track state.

Dogfood before hardening any of these into strict orchestration rules.

---

# 30. Guiding principle

The runtime should be permissive; the agents should be opinionated.

> **Encode the preferred coordination model in prompts, tools, skills, and UI affordances—not in a rigid workflow engine.**

The happy path is:

```text
human defines direction
        ↓
Track coordinator decomposes
        ↓
workers execute
        ↓
agents review
        ↓
Track integrates
        ↓
human reviews coherent Track state
        ↓
Land
```

But the system must remain flexible enough to skip, recurse, retry, inspect, or restructure that flow when the work demands it.

And for background activity:

```text
Event
  ↓
cheap context-free-ish triage
  ↓
ignore / deterministic action / wake smallest useful Thread
  ↓
wake expensive coordinator only when necessary
```

That second loop is as important as the first: it is what lets a long-running Track remain intelligent without repeatedly paying to prefill a huge high-quality model context just to discover that nothing needs doing.
