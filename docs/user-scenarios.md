# Arachne narrative scenarios

These scenarios express the intended product, using **Track** for a durable
coordinated effort and **Thread** for a conversation. New thread also supports
small standalone tasks; Convert to track preserves a thread when its work grows.
A worker remains directly addressable even when a coordinator manages the work.

The support notes below are a source audit of the October 2026 local dogfood
changes. They distinguish existing building blocks from full acceptance; they
do not claim a live webhook, remote runner, experiment, or landing was exercised.
See [design.md](design.md) and [integration-and-landing.md](integration-and-landing.md).

## 1. Find the conversation that made a PR

I am looking at a PR and remember that an agent made it. From its number,
branch, filename, or a remembered error, I want to find the Track and Thread,
read the decisions, and open the corresponding checkout. If that checkout is
gone, I can recover it without remembering a session ID or filesystem path.

Current support: Track resources, per-worker PR information, thread navigation,
Open in Zed, and checkout recovery provide the forward path from known work.
The source binds PR information to sessions and worktrees. Remaining acceptance:
universal reverse lookup from an arbitrary PR and full transcript/file/commit
search, including archived work whose PR branch has changed.

## 2. Research runs for weeks while the coordinator sleeps

I investigate whether a boundary operator improves the Grug MoE scaling curve.
One Track holds the paper, design notes, coding workers, experiments, W&B runs,
issue, and conclusions. I specify a gate: run the expensive d1024/d1280 arms
only if d512/d768 beat baseline.

The coordinator sleeps until a run completes, fails, or crosses a threshold.
It records evidence and follows the gate mechanically. A scientific judgment
appears in Needs You; an ordinary completion does not require my intervention.

Current support: durable Loom conversations, delegated workers, attached
resources, user Todos, and event-refreshed fleet/attention surfaces. Remaining
acceptance: W&B/job event adapters, persisted experiment gates and subscriptions,
and verified sleeping-coordinator wakeups. A displayed status is not proof that
an external experiment event was consumed or acted upon.

## 3. A broad effort recursively decomposes

“Make Shuttle useful for large-scale training” grows workers for IR design,
stateful scan, scheduling, transport, replay, benchmarks, debugging, and docs.
Workers can delegate further. The coordinator keeps the overall plan; each
worker returns a compact result. I can still open and talk to any worker.

A transport worker discovers a constraint the scheduling worker needs. It sends
that fact directly across threads. The coordinator accepts or rejects results,
creates follow-ups, and integrates work without rereading every transcript.

Current support: nested session hierarchy, delegation, per-thread conversation
and runtime controls, cross-thread messaging, and Track inspector navigation.
Remaining acceptance: demonstrate a complete recursive run with durable concise
upward reports, dependency-aware follow-ups, and coherent coordinator memory
across sleep/restart. The hierarchy alone does not establish those behaviors.

## 4. One feature spans Arachne and Loom

“Event mailbox UI” includes frontend work in Arachne, backend work in Loom,
a design document, and separate PRs. Its Project supplies the repositories and
references. The Track remains one effort, with a canonical accepted ref per
repository and workers checked out against the relevant repository.

Integration is per repository: Arachne changes may squash into its Track ref,
while Loom changes integrate into the long-lived Arachne development branch.
The UI should make the target explicit without making this feel like two projects.

Current support: project resource bindings, Track resources, repository/base
selection, local project launch defaults, and integration target resolution.
Remaining acceptance: complete multi-repository launch/default editing and
end-to-end validation of distinct per-repository integration policies. The
current local defaults editor saves repository/base, not the full policy model.

## 5. A rapid experiment spike becomes a clean PR stack

I try pipeline parallelism on H100s using cheap combinations of model size,
layer count, host count, stage splits, save/resume, JAX/XLA version, and offload.
Short-lived workers produce evidence. Failed runs remain inspectable without
becoming permanent attention items.

Once a configuration works, the coordinator extracts the core change, splits
checkpointing and infrastructure fixes, adds tests, and prepares a reviewable
PR stack. Exploration and engineering remain part of the same Track.

Current support: delegated threads, durable resources, integration/landing
requests, review surfaces, and separate worker versus attention state. Remaining
acceptance: experiment lifecycle adapters, comparable result summaries, and
first-class dependency/PR-stack management. A branch or PR link alone does not
represent a validated experiment result.

## 6. A broad implementation becomes reviewable pieces

A TaskCompendium spike discovers its abstraction before the final change
boundaries are clear. It includes design notes, implementation, tests, dataset
experiments, and open architectural questions. Once stable, workers extract
TaskSpec, verifier registry, importers, predicted-action support, and harness
integration in dependency order.

The original spike remains a reference resource. An integration worker can
reconcile the accepted state while the final PR structure differs from the
exploratory history. Small review fixes can be edited in the app without losing
the surrounding conversation.

Current support: attached design/files, delegation, review diff and checkout
editing surfaces, and structured integration requests. Remaining acceptance:
dependency-aware extraction and PR-stack workflows, plus verified integration
outcomes for a real broad spike. Manual editor saves must detect intervening
file changes and review must refresh after edits.

## 7. A ready worker becomes stale

A worker commits its result, passes validation, and is clean against Track HEAD.
It becomes Ready and sleeps. Another worker integrates first. A deterministic
preflight rechecks the sleeping candidate against the new HEAD. If clean it
stays asleep; if conflicting, the original worker wakes to reconcile.

Current support: candidate status and integration UI distinguish explicit
readiness from merely stopping; landing has deterministic local validation.
Remaining acceptance: durable readiness tied to source/target SHAs, automatic
re-preflight on Track advancement, and conflict-driven wakeup of the original
worker. These are required before claiming automatic stale-candidate handling.

## 8. Review arrives while nobody watches

A sleeping worker opened a PR yesterday. A reviewer leaves three comments.
The review event wakes the original worker or a cheap repair worker. Obvious
fixes proceed; a design decision goes to the coordinator and reaches Needs You
only if it needs my judgment. I return to one actionable review item.

Current support: PR/check/review status is reflected in worker and attention
surfaces, with conversation navigation and explicit review actions. Remaining
acceptance: demonstrate real webhook delivery, comment-level routing, sleeping
worker wakeup, deduplication, and escalation policy. Fleet refresh from an event
is distinct from the coordinator actually resuming to address the review.

## Local acceptance boundary

For this pass, use dummy local repositories and projects to exercise launching,
standalone-to-Track promotion, direct worker conversation, keyboard navigation,
attention actions, review, small edits, and scoped server switching. Keep remote
Loom, external experiment hooks, autonomous merges, and PR publishing out of
an acceptance claim unless separately exercised with explicit authorization.
