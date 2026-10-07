# Arachne: Bootstrap and Architecture Handoff

## What this is

**Arachne** is a macOS-first interface for managing persistent AI-assisted work across local and remote machines.

The near-term goal is not to build a general multi-agent framework. It is to build something that feels as immediate as Codex Desktop for day-to-day coding, but with a better model for:

- many concurrent workers;
- persistent long-lived tracks;
- local versus remote execution;
- multiple inference providers/accounts;
- worktrees and pull requests as navigable resources;
- event-driven automation instead of agents busy-waiting;
- escalation based on **what needs human attention**.

Arachne should use **Loom** as its execution/control-plane substrate rather than replacing Loom.

Keep Loom generic: Track semantics, project-manager guidance, Arachne resource
names, and UI attention policy belong in Arachne. Backend additions should expose
reusable session, event, channel, artifact, or repository operations without
encoding Arachne-specific conventions. The local dogfood pass requires no Loom
source changes.

The immediate priority is to get to a **self-hosting bootstrap loop** quickly: use an early Arachne build to supervise the agents that are developing Arachne itself.

---

# Reference projects

Read these before making large architectural decisions.

## Loom

https://github.com/marin-community/loom

Loom is the existing backend/control plane and should be treated as the starting substrate.

It already provides substantial machinery we do not want to rebuild:

- agent sessions;
- git worktrees;
- detached agent runtimes;
- REST/SSE APIs;
- durable conversations;
- session lifecycle;
- an explicit attention axis;
- parent/delegated sessions;
- channels;
- profiles and policy;
- GitHub integration and PR state;
- permissions;
- scheduled/event-triggered watches;
- automation-triggered runs;
- recovery/adoption;
- session archiving.

Arachne should use these primitives where possible and evolve Loom where necessary.

Important existing Loom documentation:

- Architecture: https://github.com/marin-community/loom/blob/main/docs/ARCHITECTURE.md
- UI model: https://github.com/marin-community/loom/blob/main/docs/loom-ui.md
- Profiles/MCP: https://github.com/marin-community/loom/blob/main/docs/mcp-profiles.md

## OpenAI Codex

https://github.com/openai/codex

Codex is useful both as an agent backend and as an implementation reference.

Things worth studying or reusing where appropriate:

- session/thread mechanics;
- worktree support;
- process/sandbox infrastructure;
- provider abstraction;
- authentication handling;
- terminal/process execution;
- desktop-oriented workflows exposed through the OSS core.

Codex is Apache-2.0 licensed.

Do not copy architecture simply because Codex uses it. Reuse specific implementations when they save real work.

## Agent Deck — fleet/conductor version

https://github.com/asheshgoplani/agent-deck

This is the terminal-oriented Agent Deck.

Useful concepts include:

- managing a fleet of agent sessions;
- parent/child sessions;
- its **Conductor** abstraction;
- durable child-completion inboxes;
- agent escalation;
- external “doorbells”/watchers;
- session grouping;
- worktree management;
- multi-vendor agent handling.

Especially relevant:

https://github.com/asheshgoplani/agent-deck/tree/main/docs/conductor

The Conductor is conceptually close to Arachne's longer-term **track coordinator**: a persistent logical agent supervising workers and escalating only what it cannot resolve.

Do not necessarily adopt its heartbeat/polling implementation. Arachne should prefer real events wherever possible.

Agent Deck is MIT licensed.

## AgentDeck — macOS attention UI

https://github.com/ShenAC-SAC/agentdeck

This is a different project.

Its strongest idea for Arachne is the **attention-oriented desktop UI**:

- “Needs you” as a primary state;
- native macOS notifications;
- badges;
- multiple vendors;
- sessions that continue independently of the UI;
- a cockpit that answers “where should my attention go?” rather than merely “what is running?”

Its current implementation is Electron/Bun/tmux. We probably do not want that architecture wholesale, but the interaction model is highly relevant.

It is MIT licensed.

## Other OSS

Be willing to inspect and reuse code or concepts from other OSS agent managers, terminal tools, worktree managers, remote-development tools, etc.

Specific design references to investigate:

- Gas Town's integration candidates, Refinery, and merge-queue direction;
- Stoneforge's disposable integration worktree and mechanical merge/test path;
- `wta`'s preflight conflict preview;
- Claude Code and OpenCode, alongside Codex, for the vanilla harness layer:
  thread/session operations, worktrees, resumability, and permissions.

These are inspirations, not dependencies or evidence that their internals fit
Loom unchanged.

Rule:

> Search before building, but vendor/reuse only when doing so is actually simpler or better.

When source code is incorporated:

- verify license compatibility;
- preserve required attribution/notices;
- record source repository and commit;
- prefer small isolated pieces over wholesale forks;
- maintain a `THIRD_PARTY.md` if this begins happening materially.

---

# Core product idea

Arachne should not expose Loom's entire control-plane vocabulary directly.

The user-facing model is approximately:

**Projects → Tracks → Threads/Workers → Resources → Events → Attention**

Loom implements much of the machinery underneath.

---

# Vocabulary

## Arachne

The overall product: macOS app plus the user's personal always-on control plane.

## Loom

The lower-level runtime/control plane.

Loom runs sessions, manages worktrees/runtimes, stores durable state, receives events, tracks GitHub state, etc.

## Project

A lightweight home for related Tracks and their shared context. A Project
bundles default Resources and launch settings: for example, two repositories,
a design document, a primary repository, a runner, an agent and inference
route, and a Track branch policy. It is an organizer and source of defaults,
not an execution object: it has no coordinator, mailbox, agent, or workers.
Each Track has one home Project and may explicitly bind Resources from another
Project. A Project can span repositories; it is not synonymous with a repo.

## Track

A durable unit of intent, context, resources, Todos, subscriptions,
mailbox, coordinator Thread, workers, and integration state.

Examples:

- “Implication reader”
- “TaskCompendium”
- “Arachne”
- “Science After Reproducibility”
- “Personal automation”

A track may last hours, days, or months.

It can:

- maintain durable context;
- hold resources;
- have a primary coordinating thread;
- create workers;
- receive worker results;
- receive external events;
- sleep;
- wake later;
- escalate something to human attention.

A track is **not a worktree** and is **not necessarily a continuously running model process**.
It may span multiple repositories. Each attached repository may have its own
canonical Track branch/ref for accepted code state; there is no universal
Track branch. A human-facing checkout is optional and reconstructible from its
ref. New track opens a chat immediately (setup card above the chat box,
gone at launch). New thread creates a standalone
one-off conversation; Convert to track promotes it when the work grows, keeping
its history and resource associations. Existing internal `topic` identifiers
remain compatible with Loom.

Long term, think of it as a durable actor with a mailbox that occasionally invokes an LLM.

## Thread

The conversation/UI surface through which the user interacts with one execution context.

Initially, a Thread can map closely to a Loom session.

A track's coordinator has a thread. Workers also have threads.

## Worker

An execution acting on behalf of a parent thread/track.

“Worker” describes hierarchy, not intelligence.

A worker may be:

- cheap GLM;
- normal Codex;
- high-effort Codex;
- Claude;
- another smart agent that itself creates workers.

## Resource

A durable object relevant to work.

Resources include:

- repository;
- worktree/checkout;
- pull request;
- file;
- folder;
- design document;
- artifact;
- eventually W&B runs/projects, datasets, issues, services, etc.

Resources should have identity independent of the session that happened to create them.
Bindings to Projects, Tracks, and Threads should preserve that identity and
show where each binding came from.

## Todo

A durable, cross-track action for the person. The user Todo list is separate
from a Track's plan/backlog and from worker-internal checklists. Integration
and review decisions may create user Todos.

## Event

An immutable fact that something happened. Its Source is the emitter; a
Subscription selects which Events a Track or Thread cares about; its Mailbox
holds pending Events/messages durably; a Wake is the decision to invoke an
agent because of queued Events. Delivery alone does not imply a Wake.

Examples:

- human message;
- another Thread's note or selected result;
- worker completion;
- worker escalation;
- GitHub review;
- CI state change;
- W&B alert;
- heartbeat, timer, or cron;
- webhook;
- integration conflict after Track state advances;
- a runner becoming available.

Events can be coalesced, handled deterministically, or held until a later
Wake. This is the common mechanism behind watches, scheduled automations,
cross-thread delivery, and manual messages, without agent busy waiting.

## Attention

Something that actually requires the human.

This is deliberately different from:

- a worker needing help;
- a worker finishing;
- a PR changing;
- an agent becoming idle.

A child worker should often escalate to its smart parent first. The parent may resolve the issue without interrupting the user.

---

# First architectural principle: one authoritative control plane

Do **not** replicate Loom state between laptop and cloud.

There should be one authoritative personal Loom deployment that stays online when the Mac sleeps.

Initial host:

**DGX Spark at `100.121.8.110` over Tailscale**

Initial topology:

```text
                       Arachne.app
                          Mac
                           │
                           │ REST/SSE over Tailscale
                           ▼
                 Personal Loom instance
                    100.121.8.110
                           │
                           │
                     DGX runner
```

Later:

```text
                      Personal Loom
                     always-on control
                  /          |          \
                 /           |           \
          DGX runner    OA cloud       Mac runner
                                        when awake
```

The Mac eventually becomes a runner and local-resource gateway, not another authoritative Loom instance.

This avoids distributed-state/replication problems around:

- conversations;
- track state;
- attention;
- worker ancestry;
- resource bindings;
- automations;
- event delivery.

---

# Deployment philosophy

Keep it boring.

Good:

- Tailscale;
- SSH;
- systemd;
- Docker Compose where convenient;
- SQLite;
- filesystem backups.

Bad, unless genuinely forced later:

- Kubernetes.

Eventually Arachne should offer a setup flow that can install/update the personal Loom control plane onto an always-on machine.

For example:

```text
Arachne → Set up control plane
        → choose Tailscale/SSH machine
        → install Loom service
        → initialize state/auth
        → register endpoint
```

This is not required for the first prototype. Manual deployment is fine.

External GitHub/W&B webhooks cannot hit a Tailscale-only service directly, so later add either:

- narrowly exposed HTTPS ingress; or
- a tiny public event relay.

Do not solve that before Arachne itself works.

---

# Desktop architecture

Use **Tauri 2**.

Frontend: **Vue 3**.

Reasons:

- Loom's current SPA is Vue 3, making reuse straightforward;
- Tauri uses macOS WKWebView rather than bundling Chromium;
- native backend is Rust, matching Loom;
- native needs are fairly small.

Expected native responsibilities:

- Keychain;
- notifications;
- Dock badges;
- Zed launching;
- PTY/process support;
- SSH/tunneling support;
- filesystem/resource access;
- eventually local runner lifecycle;
- app updating.

Do not build the product in Swift just for native purity.

Do not bundle desktop automation, LibreOffice, a browser, or an IDE unless actual usage creates a need.

---

# Loom development strategy

Maintain a **long-lived Arachne branch against Loom**.

Suggested branch name:

```text
arachne
```

This is preferable to immediately trying to upstream every experimental abstraction.

Rules:

1. Regularly merge/rebase current Loom `main`.
2. Keep Arachne changes clear and topical.
3. Avoid gratuitous divergence.
4. Upstream generally useful Loom improvements once their abstractions stabilize.
5. Experimental schema/API changes can remain on the Arachne branch initially.
6. Prefer additive API changes.

Arachne itself should probably be a separate repository/app, while the Loom branch carries backend extensions needed by Arachne.

Avoid building a second orchestration backend inside the Arachne repo.

---

# Bootstrap development is the priority

Before building the grand architecture, get a real coding loop working.

## Bootstrap milestone

From Arachne on the Mac:

1. Connect to personal Loom on `100.121.8.110`.
2. See real Loom sessions.
3. Launch a real coding task.
4. Watch the conversation live.
5. See lifecycle/attention.
6. Identify the repository/worktree associated with the session.
7. Click **Open in Zed**.
8. Zed opens the exact checkout the agent is using on the DGX.
9. Interact with the session from Arachne.
10. Start using Arachne to implement subsequent Arachne work.

This milestone should happen **before** designing generalized worker scheduling, resource graphs, Mac runners, or webhook infrastructure.

The system should begin teaching us what to build next.

---

# Initial Arachne UI

Do not replicate Loom's existing operations dashboard.

Arachne should feel like a coding cockpit.

Roughly:

```text
┌─────────────────┬───────────────────────────────┬────────────────────┐
│ PROJECTS/TOPICS │ Track · current Thread        │ Threads            │
│ Arachne         │                               │ Resources          │
│   UI            │      conversation             │ Integrations       │
│   Integration   │                               │ Todos              │
│ Marin           │                               │                    │
│   TaskCompendium│                               │                    │
│ ...             │ [composer__________________]   │                    │
└─────────────────┴───────────────────────────────┴────────────────────┘
```

The default **Tracks home** screen should answer:

> What is happening across everything, and what needs my attention?

Tracks home aggregates all Tracks across all Projects. The sidebar groups
Tracks beneath Projects, with **Tracks [+]** opening the optional creation
form in a sheet or popover for the current Project. Every aggregate row names
its Project and parent Track and opens that Thread. Clicking a Project shows
the same status groups filtered to its Tracks; it does not open a Project
chat. Inbox may remain a separate attention/review tab, but Tracks home still
includes all cross-Project Needs You items.

Selecting a Track opens its **coordinator conversation** in the main pane,
making the coordinator the Track's voice. A Track click always lands on
the coordinator — no restore of the last-opened worker Thread. Clicking
the already-selected Track keeps the coordinator (a no-op). The Threads
tab and chat header provide an explicit Coordinator action. The main pane
should almost always be a chat; Tracks and Threads are levels of context,
not peer destinations.

The right pane is the Track inspector: **Threads | Resources | Integrations |
Todos**. Threads lists the coordinator and workers and switches the main chat;
New Thread belongs there. Resources includes Track refs and checkouts plus
distinguishable Thread resources, with Open in Zed prominent there or in the
header. Integrations shows the Track-owned candidate queue, readiness,
conflicts, review, and Integrate actions. Todos is a Track-filtered view of
the durable cross-track user Todo list, distinct from a Track plan or worker
checklist. Events should feed the appropriate tab and Attention, not become a
top-level tab until a real workflow needs one.

The aggregate home keeps **Needs You**, **Working**, **Ready to Integrate**,
and de-emphasized/collapsible **Waiting / Resting**. A deliberately opened
**Track Overview** scopes those same groups to one Track and adds summary and
resources. It is accessible from the Track/coordinator header, not the
default destination. The Ready group contains only verified integration
candidates; an idle or completed worker is not automatically Ready.

The New thread sheet starts a standalone one-off conversation. New track
opens a chat immediately — setup choices (title, repo/base, inherited
project resources) sit in a card above the chat box and disappear at
launch; the first send delivers the message and launches the track in the
selected or default Project. Convert to
track promotes a thread when it needs durable coordination and workers. Track
selection always opens its coordinator chat.

Example:

```text
NEEDS YOU

  TaskCompendium
  Worker needs decision about verifier API
  3m ago

  Arachne
  PR review has substantive feedback
  11m ago


WORKING

  Arachne bootstrap         Codex · DGX
  AAII cleanup              GLM · OA Cloud


READY TO INTEGRATE

  Arachne · resource strip   clean against Arachne@789abc


WAITING / RESTING

  PR #1842                  waiting for review
  Hero nightly              waiting for W&B
```

The macOS AgentDeck reference above is useful here.

---

# Code access

Every thread should expose its most relevant resources immediately.

Example:

```text
[Open in Zed ▾]
                   Repository   marin
                   Pull request PR #1842 · open · CI success
                   Branch       implication-reader
                   Checkout     /path/to/worktree
                   [Open Terminal]
```

Keep the editable-checkout action visible in the thread header. Put verbose
repository, PR, branch, path, change-summary, and secondary Terminal details
behind its adjacent disclosure instead of reserving a permanent row for
metadata that will usually be truncated.

A central product invariant:

> If Arachne shows something referring to code, it should be possible to reach an editable checkout of that code in one action.

This applies from:

- thread;
- PR;
- attention item;
- review comment;
- CI failure;
- diff.

Do not make the user remember which agent/session generated which worktree.

---

# Zed integration

Zed is the primary external editor target.

Local worktree:

```text
Open in Zed
→ open local filesystem path
```

Remote worktree:

```text
Open in Zed
→ open Zed remote project over SSH
```

Zed supports SSH remote development, so no source-file synchronization system should be built.

For bootstrap, getting a Loom-created worktree on `100.121.8.110` into Zed with one click is a major goal.

If a PR's old worktree has disappeared:

```text
PR #1842
No active checkout

[Create worktree & Open in Zed]
```

Loom should recreate a checkout from the PR branch/head, register its association, and open it.

---

# Resources

The data model should eventually distinguish resources explicitly.

## Binding and defaults

Resources can be bound at three scopes: **Project → Track → Thread**. A Project
supplies reusable repository, design document, and other Resource bindings
plus launch defaults. A Track inherits those bindings, adds or overrides its
own, and may hide one for its own context without removing it from the
Project. A Thread inherits the effective Track context and adds its own
worktree, PR, file, or artifact. The UI should show the origin of each binding
and avoid duplicating the underlying Resource object.
Project bindings can continue to supply shared references to existing Tracks;
Track additions, overrides, and hides remain in effect.

New Tracks inherit the Project's current runner, agent, inference route,
primary repo, branch creation policy, and integration policy unless the user
overrides them. Record the chosen execution settings and canonical refs on
the Track. Changing Project defaults must not silently change active Track
branches, runners, or integration targets; applying a change to an existing
Track is explicit and reviewable. Inheritance of a Resource is context, not
ownership of a checkout or permission to edit it.

## Repository

Logical source repository.

Example:

```text
marin-community/marin
```

## Worktree

Concrete mutable checkout.

Properties may include:

```text
repository
branch
base revision
runner/host
filesystem path
dirty state
associated PRs
active users
lifecycle
```

Most worktrees should be ephemeral.

Possible lifecycle:

```text
ephemeral
attached
external
```

- **ephemeral**: worker-owned; safe to remove when done;
- **attached**: explicitly retained by a track;
- **external**: existing checkout Loom did not create.

## Pull request

A first-class resource, not just metadata on a worktree.

A PR may outlive its checkout.

Typical relationship:

```text
repository
   ↑
 worktree
   │
 branch
   │
   ↓
 pull request
```

But model these as relationships rather than strict containment.

## Files/folders

These may be local or remote resources.

Example:

```text
~/Obsidian
location: David's Mac
access: read/write
```

This becomes relevant once the Mac runner exists.

## Design documents

A design document is also a first-class resource.

It might physically be a file in a repository branch, but logically it should be referenceable independently.

Example:

```text
kind: design_document
title: Arachne architecture
backing:
  repository: arachne
  ref: arachne/bootstrap
  path: docs/design.md
```

Do not build a special document database initially. A typed file resource is enough.

---

# Worktrees and resource usage

The user should almost never need to think about worktrees during ordinary task launch.

Normal:

```text
task
→ Loom creates worktree
→ agent works
→ PR/merge
→ worktree archived
```

When a checkout becomes important, allow:

```text
Keep worktree
```

or equivalent.

It is then attached to the track as a durable Resource.

Also track which worker is using a resource.

Eventually use something like leases/bindings:

```text
worker A --write--> worktree X
worker B --read--> worktree X
```

Good default:

> one active writer per worktree; multiple readers allowed.

When another writer wants the same state, prefer creating another worktree.

---

# Tracks and workers

A track can contain a coordinating thread and workers:

```text
Implication reader                   Track
│
├── coordinator                      Thread
│
├── implement reader                 Worker
│   ├── investigate GDN API          Worker
│   └── add tests                    Worker
│
├── benchmark variants               Worker
│
└── monitor PR                       Worker / automation
```

A worker may itself spawn children.

Do not impose artificial depth limits apart from practical resource/budget controls.

Parent-child relationships should be durable and visible.

The terminal Agent Deck's Conductor is a useful reference for this shape.

---

# Worker results

A child finishing should not require its parent to reread the full child transcript.
Finishing is not the same as being ready to integrate. A coding worker should
normally stabilize/commit its result, run required validation, summarize it,
and preflight mergeability against a specific Track ref before becoming Ready.
Readiness records the target revision. The worker may sleep while its separate
integration state remains Ready, stale, conflicting, or integrated.

Eventually produce a structured handoff:

```text
status
summary
resources created/modified
commits
pull requests
open questions
recommended next action
```

The complete transcript remains available as evidence/history.

Loom's existing channels/results can be the initial transport. Do not block bootstrap on a perfect handoff schema.

---

# Tracks should sleep

Long-term, a track should not require a continuously-running LLM.

Think:

```text
durable state
+ mailbox
+ subscriptions
+ resources
+ history
```

When an event matters:

```text
event
→ subscription / durable mailbox
→ coalesce or handle mechanically, if possible
→ wake coordinator only when reasoning is needed
→ reason / act / delegate
→ update state
→ sleep
```

This is preferable to continuously consuming a session or having agents poll.

---

# Events

The control plane records an immutable **Event** from a **Source**, routes it
through a **Subscription** to a Track or Thread's durable **Mailbox**, and
makes a separate **Wake** decision. Sources include manual messages, other
Threads, worker completion, integration conflict, GitHub/CI/PR review, W&B,
heartbeats/timers/cron, and runner availability. A delivery can update state,
coalesce with related Events, or remain pending without invoking an LLM.
Repeated delivery must be idempotent. This vocabulary should unify the
existing watches and automations rather than creating a second scheduler.

---

# No busy waiting

Avoid:

```text
gh pr checks
sleep 30
gh pr checks
sleep 30
...
```

Instead:

```text
GitHub event
→ Loom
→ relevant mailbox
→ coalesce or update state
→ wake relevant thread/track when needed
```

Likewise:

```text
W&B alert
CI transition
PR review
experiment completion
timer
runner reconnect
```

Loom already has:

- watches;
- cron triggers;
- session-event triggers;
- automation runs;
- GitHub polling/state;
- channels.

Extend these toward the event model instead of creating a parallel scheduler.

---

# Automations

An automation should eventually be understood mostly as a **subscription that sends events to a track**, not as a completely separate class of thing.

Examples:

```text
Every morning
→ personal track
→ scan inbox
→ update Obsidian todos
```

```text
PR review submitted
→ Arachne track
→ cheap worker handles obvious comments
→ escalate difficult comments to coordinator
```

```text
W&B run alerts
→ research track
→ inspect failure
```

A Mac-local resource such as the Obsidian vault should naturally constrain execution:

```text
requires: ~/Obsidian
runner availability: David's Mac
```

If the Mac is asleep:

```text
Waiting for David's Mac
```

Do not silently copy the folder to the cloud.

---

# Local vs remote execution

Eventually, launch should separate three axes.

## Runner

Where does execution happen?

Examples:

```text
David's Mac
DGX Spark
OA Cloud
```

## Agent runtime

What coding agent?

Examples:

```text
Codex
Claude
Pi
OpenCode
```

## Inference route

Whose model/account/credentials?

Examples:

```text
David · Codex
OpenAthena · OpenAI
OpenAthena · GLM
```

These must eventually be separable.

Examples:

```text
Runner:      This Mac
Agent:       Codex
Inference:   David · Codex
```

```text
Runner:      DGX
Agent:       Codex
Inference:   OpenAthena · GLM
```

Loom profiles may temporarily encode these combinations during bootstrap.

Do not make **Profile** the user-facing central concept.

---

# Mac runner

This is important, but comes after bootstrap.

Eventually Arachne.app should operate a local Loom runner/resource gateway.

It should provide:

- local agent execution;
- local worktrees;
- local filesystem resources;
- PTYs;
- sandboxing;
- heartbeat/connectivity;
- lifecycle reporting;
- reconnection after sleep.

It should establish an outbound connection to the personal control plane rather than requiring the control plane to dial arbitrarily into the laptop.

Again:

> Mac runner != Mac Loom database.

---

# Attention and escalation

Use explicit escalation levels conceptually.

A worker may say:

```text
needs parent
needs stronger model
needs resource
needs human
```

Not all failures belong in human Attention.

Example:

```text
GLM worker cannot solve issue
→ escalate to parent coordinator
→ parent retries with high-effort Codex
→ resolved

David sees nothing
```

Only if the hierarchy cannot resolve it:

```text
→ human Attention
```

This is a central product feature.

---

# Cross-thread knowledge transfer

There are real cases where Thread A knows something Thread B needs.

Do not build a generalized multi-agent communications platform yet.

Provide one durable delivery primitive through both **Send to Thread…** in
the UI and an agent-callable Loom tool. Both routes should use the same
underlying operation and return a delivery receipt. The source identity comes
from the authenticated human/session, not caller-supplied display text;
Loom authorizes the destination and records provenance and an idempotency key.
Agents should be able to discover destinations they may reach. Delivery is an
Event and may remain queued without an immediate LLM Wake.

Initial forms:

- send human-written note;
- send selected message;
- send resource/artifact;
- ask source thread to summarize what it knows about X and deliver that summary.

Destination receives a durable inbound item with provenance.

The existing Arachne UI sends a human note through Loom's channel-message
operation. Loom's `channel_send` tool already lets an agent send to channels
within its authorized session tree or subscriptions; a first-class
cross-thread tool should make destination selection and delivery semantics
explicit without broadening access silently.

Agents are free to coordinate through other means when available; Arachne does not need to model every emergent communication strategy.

---

# Boring coordination primitives that matter

Eventually support these explicitly.

## Durable mailbox

Messages/events persist until processed.

## Idempotency

Duplicate webhook delivery should not duplicate work.

## Event coalescing

Five PR updates in 30 seconds should often become one wake containing current state.

## Claims / leases

Track who owns responsibility for work or mutable resources.

## Reconciliation

After crashes/restarts, compare durable desired state to actual processes/resources and repair.

Loom's existing orphan/adoption model is a good start.

## Backpressure

Agents must not spawn unbounded workers.

Limits may exist per:

- track;
- runner;
- inference account;
- provider.

## Integration

Integration belongs to the Track. Each repository attached to it may have a
canonical ref for accepted state. Worker results are candidates against that
ref, and the Track view should expose their queue and status. The normal flow
is worker → Track; nested workers may integrate recursively when useful.
**Integrate** absorbs work into Track state. **Land** separately moves accepted
Track state upstream — non-PR strategies target the primary local checkout's
currently checked out branch (`main` as fallback); `open-pr` targets the
remote's default branch.

Preflight conflict preview (for example, `git merge-tree`) and readiness are
relative to a target revision. When the Track ref advances, Loom should
deterministically recheck sleeping Ready candidates. Clean ones get a refreshed
readiness revision and stay asleep;
new conflicts emit Events and preferentially wake their original workers to
reconcile. Worker lifecycle and integration state remain independent.

The operation carries source, target, strategy, revision, and validation
policy. Strategies include squash, merge, rebase, cherry-pick, PR, and ask
coordinator. A clean, unambiguous integration should use a disposable worktree:
prepare, apply, validate, atomically advance the Track ref, then clean up.
Failure must not disturb the canonical checkout/ref. Conflicts, failing tests,
ambiguity, or product judgment can wake the original worker, coordinator, or
an integration worker. Do not force an LLM turn for a clean mechanical merge,
or force the coordinator to absorb every difficult one.

---

# Security model

Eventually, resource attachment should also define capabilities.

Example:

```text
worker: fix-reader

resources:
  marin checkout        read/write
  design.pdf            read

network:
  GitHub                allowed
  other                 ask/policy
```

A local agent should not automatically receive all of `$HOME`.

Use native/server credential stores.

Mac credentials belong in Keychain where appropriate.

Do not store sensitive credentials in frontend localStorage.

Do not let this block the initial remote-only DGX prototype, but avoid architecture that assumes unrestricted local access.

---

# What not to build yet

Explicit non-priorities for bootstrap:

- Kubernetes;
- control-plane replication;
- desktop automation;
- general browser automation/UI;
- built-in browser;
- office suite/document rendering;
- IDE functionality beyond integration;
- full Mac runner;
- generalized multi-agent chat;
- graph database infrastructure;
- sophisticated provider scheduler;
- public webhook ingress;
- elaborate swarm visualization;
- generalized track memory system.

Use the simplest version compatible with future extension.

---

# Suggested implementation order

## Phase 0 — bootstrap

Goal: make Arachne useful for building Arachne.

1. Create Arachne repository.
2. Create long-lived `arachne` branch in Loom.
3. Deploy Loom to DGX Spark `100.121.8.110`.
4. Create Tauri 2 + Vue 3 app.
5. Connect to Loom REST/SSE.
6. Authenticate.
7. Show Loom sessions.
8. Open a session/thread.
9. Render conversation.
10. Send user input.
11. Launch a new coding session.
12. Surface Loom lifecycle/attention.
13. Resolve its repository/worktree.
14. Implement **Open in Zed** against the DGX worktree.
15. Add terminal access if cheap.
16. Begin using Arachne for subsequent development.

Stop and evaluate here.

## Phase 1 — make it an actual product

Add:

- attention-first home;
- native notifications;
- compact code access and checkout details;
- PR resources;
- reliable “open the code” behavior;
- basic Tracks;
- Project grouping and inherited resource/launch defaults;
- resource attachment;
- design-document resources;
- keep/attach worktree;
- Send to Thread.

## Phase 2 — execution routing

Add:

- runner abstraction;
- Mac runner;
- first-class inference routes;
- runner/agent/inference launch controls;
- resource locality and availability.

## Phase 3 — durable coordination

Add based on observed usage:

- sleeping/waking coordinators;
- worker delegation UI;
- structured worker returns;
- subscriptions;
- webhook/event routing;
- W&B;
- PR review handling;
- scheduled automations;
- coalescing/idempotency;
- escalation hierarchy;
- leases;
- budget/concurrency policy.

---

# Phase 0 acceptance test

A successful bootstrap demo is:

1. Arachne is running on the Mac.
2. It connects over Tailscale to Loom on `100.121.8.110`.
3. User launches a real task in a real repository.
4. Loom creates a worktree and starts the actual coding agent on the DGX.
5. Conversation streams live into Arachne.
6. Session state is visible.
7. If the agent requests input, Arachne clearly surfaces it as needing attention.
8. User responds from Arachne.
9. User clicks **Open in Zed**.
10. Zed opens the exact remote worktree being edited by the agent.
11. User can inspect/edit the same files.
12. Associated git/PR state is visible or navigable.
13. This workflow is good enough to use for developing the next Arachne feature.

At that point, do **not** automatically proceed down the roadmap.

Use Arachne for real work first.

The architecture after Phase 0 should be informed by actual recurring friction, especially around:

- track creation;
- worktree persistence;
- attention;
- worker delegation;
- local versus remote execution;
- provider switching;
- event-driven wakeups.

The guiding principle is:

> Build enough architecture to preserve the direction, but get Arachne into its own development loop before solving the whole problem.

## October 2026 local dogfood note

The product calls durable coordinated efforts **Tracks**. Standalone Threads
remain available for small one-off tasks and can become Tracks explicitly.
API names and durable tags using `topic` keep their existing wire semantics.
The narrative scenarios in [user-scenarios.md](user-scenarios.md) separate
current source support from the remaining event-driven coordination work.
Named Loom server switching and local project launch defaults are conveniences;
remote Loom validation is outside this dogfood pass.
