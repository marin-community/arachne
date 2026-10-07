# Arachne design

Status: local dogfood implementation, October 2026. This is the short, living
design document for the app. The broader intent and acceptance test are in
[handoff.md](handoff.md); integration details are in
[integration-and-landing.md](integration-and-landing.md).

## Purpose

Arachne is a macOS coding cockpit for long-running work. It helps one person
supervise tracks and workers, notice decisions that need attention, inspect
their code, and move accepted work forward. Loom owns execution, durable
conversations, worktrees, artifacts, permissions, and event delivery. Arachne
should present those capabilities as a small set of understandable actions.

The bootstrap test is concrete: launch a coding worker, watch its conversation,
inspect its changes, open the same checkout in Zed, respond to attention, and
integrate its result into a track. The Mac-local path is useful for development;
remote Loom remains a later acceptance target.

## User model

- **Project:** a home for related Tracks and a reusable bundle of resource and
  launch defaults. It has no coordinator, mailbox, agent, or execution state.
  A Track has one home Project but may reference resources from elsewhere.
- **Track:** the durable unit of intent, context, resources,
  subscriptions, mailbox, coordinator thread, workers, and integration state.
  Its coordinator may sleep. Accepted code state is a ref per attached
  repository, not one branch shared across repositories.
  Creation offers a short title and a body that becomes the agent goal; either
  can be left blank when the other or an attachment supplies the intent.
- **Thread/worker:** one Loom session and its conversation. A standalone thread
  handles a one-off task without a coordinator or a Track. A delegated worker
  belongs to its parent’s Track. Hierarchy records responsibility; it does not
  determine Git ancestry.
- **Resource:** a repository, checkout, PR, document, file, or artifact with a
  stable identity independent of the session that first mentioned it.
- **Todo:** a durable, cross-track action for the person. A track's plan and a
  worker's internal checklist are separate from user Todos.
- **Attention:** a reason for the person to act. An idle worker is not an
  attention item; a blocked decision is.

The main screen answers “what needs me?” Track and thread views then show the
conversation alongside code and resources. Every code reference should lead to
an editable checkout with one action when that checkout is available.

With no Project or Track selected, **Tracks home** is an aggregate fleet view
across all Projects and Tracks. It shows Needs You first and prominently (or
“Nothing needs you”), then Working, Ready to Integrate, and collapsed Waiting /
Resting. Every row names its Project and parent Track and opens its Thread
conversation. Selecting a Project opens the same status groups filtered to that
Project's Tracks. A
Project home is an aggregate view, not a Project conversation.

Selecting a Track opens a **chat**, normally its coordinator Thread. The
coordinator is the Track's voice, not one item buried in a dashboard. On a
first visit, open the coordinator; on later visits, restore the last Thread
opened within that Track if it is still available, otherwise return to the
coordinator. Clicking an already-selected Track keeps the current Thread.
The coordinator row in the Threads inspector and a header action provide an
explicit way back to it. The legible hierarchy is global home → Project →
Track → Thread, while the main pane remains a conversation and composer
whenever a Track is open.

The right pane is a **Track inspector** with tabs:

- **Threads:** coordinator and worker fleet with their attention, working,
  readiness, and resting states. Selecting a row switches the main chat.
  Place New Thread here.
- **Resources:** Track resources and refs, with Thread resources distinguished
  when viewing a worker. Keep Open in Zed prominent here or in the chat header.
- **Integrations:** the Track-owned candidate queue, preflight/validation
  state, review links, and Integrate actions. Land remains a separate Track
  action toward upstream.
- **Todos:** a Track-filtered view of the durable cross-track user Todo list.
  Track plans and worker-internal checklists remain separate.

An **Overview** action in the Track or coordinator header opens the scoped
dashboard deliberately: Needs You, Working, Ready to Integrate, Waiting /
Resting, summary, and resources. It is not the default Track destination.
Events feed these surfaces and Attention; do not add an Events inspector tab
until a concrete workflow calls for it. The chat header stays compact: Track,
current Thread, runtime, and a small set of relevant actions.

The sidebar groups Tracks under their home Projects. A Track is a folder
over its coordinator thread: subthreads render beneath the coordinator
row, and the folder is collapsed by default — opening it is a deliberate
chevron click, and selecting a nested subthread expands its ancestors so
the selection stays visible. Archived Tracks
(archived leaders) disappear from the list by default — archiving in Loom
tears down the checkout, so a done Track is history, not fleet — and a
filter toggle in the toolbar shows them again, dimmed, with their
descendant threads. Archived child threads keep a live Track's shape
(stay counted, dimmed) rather than vanishing. **Tracks [+]** in a
Project opens the new-track chat preselected to that Project; it does not
permanently occupy sidebar space. Inbox may remain a separate tab, but its purpose is
cross-Track human attention and reviewable events, rather than an
unrelated second list. The aggregate Tracks home still shows Needs You even
when Inbox exists. Ready to Integrate requires explicit verified candidate
state from Loom; a stopped or sleeping worker is not sufficient evidence.

**New thread** starts a standalone conversation for a one-off task (a form
sheet). **New track** opens a chat immediately: the same shape as a live
thread — header, conversation, composer — with the setup choices (title,
repository and base, inherited project resources) in a setup card above
the chat box, where the conversation will land. The setup disappears once
the model starts. Loom's launch needs the first message, so the first
send from the chat box both delivers that message and launches the Track;
the chat box keeps what belongs to a chat — message text, attachments,
@-mentions of existing resources, and the launch-config pills
(profile/agent/model/effort) beside the send button. **Convert to
track** promotes an existing standalone thread, preserving its conversation;
its children and resources remain associated with it. The Tracks list expands
through delegated threads at arbitrary depth. Older unmarked top-level
conversations retain their legacy Track interpretation without rewriting Loom
history. Internal `topic` command names, IDs, and resource tags remain compatible.
Delegating from a thread creates a child in the conversation tree; within a
Track, its Git base comes from that Track’s accepted branch.

Tracks are ordered by **last user message time** — the newest user message
anywhere in the track's subtree (coordinator or worker). A busy agent streams
constantly, and Loom restamps `last_activity_at` on every frame, so activity
ordering makes tracks jump around while you watch; ordering by when a person
last steered the track keeps the list still while work runs. Worker triage
lists (Needs You, Working, the Track inspector's threads) keep their own
attention- and activity-based ordering — they are about workers, not tracks.

## Project defaults and resource inheritance

A Project can bind multiple repositories, design documents, and other
Resources. It can also set defaults for runner, agent, inference route, primary
repository, Track branch creation policy, and integration policy. A new Track
inherits these unless its creation form overrides them. Its Threads inherit
the effective Track context and may bind their own worktree, PR, file, or
artifact. The effective view is **Project resources/defaults → Track additions
and overrides → Thread-specific bindings**. Show each binding's origin so the
user can tell what came from the Project, Track, or Thread; allow a Track to
hide an inherited resource without deleting it from the Project.

Project resource bindings may continue to supply shared references to existing
Tracks; the Track's own additions, overrides, and hides remain in effect.

Resource identity is shared across these scopes. Inheritance grants context
and a default binding, not ownership of another Track's checkout or permission
to edit it. A Track can refer to a Resource from another Project explicitly.
At creation, record the selected runner, agent, inference route, branch policy,
and canonical refs on the Track. Updating Project defaults can affect new
Tracks, but must not silently retarget active Track branches, runners, or
integration targets. Explicitly applying a changed default to an existing
Track is a reviewable operation.

Worktrees are Resources, not Tracks. A Track may retain a checkout for human
editing, but its canonical accepted state is the repository ref. New coding
workers normally fork from that ref for their repository. This keeps
conversation hierarchy independent from Git ancestry.

## Resource slice

The Resources panel's live set is the full spec'd set (User model): the
track's **repository**, the **pull requests** of every thread in its subtree
(a worker's PR is the track's result too), the **GitHub issues** its subtree
claims or sourced, and the **checkout**. These rows are derived from Loom's
own data — the fleet snapshot's per-branch PR status and `issues.board` — so
they never go stale against the manifest. Loom records `issue_added` and
`pr.*` events on the working branch, and any frame on a session topic
re-snapshots the fleet, which refreshes the panel. Recovery is **an action on
the checkout row**, not a resource: when the worktree is present the row
opens it in Zed; when archive has removed it, the same row recovers it via
`repos.worktrees.ensure`.

For bootstrap, attach a small manifest to the coordinator's Loom branch as a
versioned artifact. Each entry has a kind, title, repository, and backing
locator: a branch/ref and relative path for a file, or a URL for a PR or
issue. The
manifest points to existing objects; it does not copy a repository or document
into Arachne. Revision checks protect concurrent edits.

The first manual attachments are **design documents** and **files**. A coding
track's design document is a normal file on its accepted branch, such as this
one. Arachne previews it through Loom's server-side worktree API and opens the
same file in Zed. The binding survives session archive, although preview and
editing require an active checkout. Loom artifacts remain appropriate for
agent-authored reports and standalone versioned documents.

Typing `@` in a thread message, a new thread goal, or a new track's
chat box offers
attached resources.
The visible mention is readable, while Arachne resolves its stable ID against
the current manifest when sending and includes the backing locator in the
agent's prompt. Removing the visible mention removes that resource from the
send. New tracks can cite resources from existing tracks before they have
bindings of their own.

File attachments use Loom Scratch. At launch they seed the new session's
checkout; in an existing ACP thread, Arachne uploads them to that session and
passes Loom resource links with the prompt. The composer also accepts dropped
or pasted files, including screenshots and other raster images; image
attachments show a local thumbnail before send. Vision-capable agents such as
Codex can consume those image resources, while other agents retain the same
file attachment fallback. Scratch files are also track resources from the
first moment: a launch attachment is recorded in the new track's resource
manifest as a File binding at `scratch/<name>` — the track starts with what
you attached, listed in the Resources panel and mentionable — while the
bytes themselves stay Scratch session inputs (the preview reads the
worktree, so it follows the file, not a commit). Scratch files are session
inputs, while track resource bindings are durable references to repository
files and artifacts.

Later resource types should be added only when a real workflow needs them.
The next likely additions are a retained integration checkout and a PR that
outlives its checkout — the PR kind already exists in the manifest; agents
attach the ones the track should remember durably (skills/resources.md).
One active writer per mutable checkout remains the
default; other workers get isolated worktrees.

## Integration

The Track owns integration candidates from its workers, with a queue and
status visible in the inspector's Integrations tab. **Integrate** absorbs a
candidate into the Track's accepted ref for the relevant repository. **Land**
moves that accepted state upstream — non-PR strategies target the primary
local checkout's currently checked out branch (`main` as fallback), while
`open-pr` targets the remote's default branch. A nested
worker may integrate through its parent, but worker → Track is the normal
visible path.

Worker lifecycle and integration lifecycle are separate. Stopping does not
make a coding result Ready. The worker normally commits or stabilizes its
changes, runs required validation, summarizes the result, and preflights it
against a specific Track revision. Readiness records that revision (for
example, `Ready · clean against Arachne@789abc`); the worker can then sleep.
When the Track ref advances, Loom should deterministically recheck sleeping
Ready candidates. Clean candidates get an updated readiness revision and stay
asleep. A new conflict emits an event
and preferentially wakes the original worker to reconcile.

Integrate is a structured operation: source, target, strategy, target revision,
validation policy, and result. Strategies include squash, merge, rebase,
cherry-pick, PR, and ask coordinator. A preflight conflict preview (for
example, `git merge-tree`) informs the UI before an attempt; the actual apply
and validation remain authoritative. For a clean,
unambiguous candidate, Loom can prepare a disposable worktree, apply the
strategy, validate, atomically advance the Track ref, and clean up without an
LLM call. Failed attempts leave the canonical checkout and ref intact.
Conflicts, failing validation, ambiguity, and product judgment route to the
original worker, coordinator, or an integration worker as appropriate. A
request is not a success state; Arachne shows the verified commit or PR result.

## Events and attention

- **Event:** an immutable fact that something happened.
- **Source:** the emitter of Events.
- **Subscription:** a rule for which Events a Track or Thread receives.
- **Mailbox:** durable pending Events and messages for a Track or Thread.
- **Wake:** a decision to invoke an agent because of queued Events.

Timers, cron, GitHub webhooks, PR review and CI changes, W&B alerts, worker
completion, integration conflicts after a Track ref changes, cross-thread
notes, runner reconnects, and manual messages all enter this model. Delivery
does not imply an LLM wake: Loom may coalesce Events, handle them
deterministically, or leave them queued until a later wake. This replaces busy
waiting without making every notification a model turn. Only unresolved human
decisions become Attention; integration or review decisions can create Todos.

Cross-thread knowledge transfer stays small: **Send to Thread…** is both a UI
action and an agent-callable Loom tool backed by the same durable delivery
operation. Either can deliver a note, selected message, resource or artifact,
or generated summary. Loom resolves the source from the caller's identity,
checks the destination's access policy, and records provenance and an
idempotency key. An agent can discover destinations it may send to; it cannot
claim to speak for an unrelated Thread. Delivery creates an Event, not an
automatic LLM Wake. A general worker-chat protocol is outside the bootstrap
scope.

## Near-term acceptance

1. A track can bind and preview this document without depending on which
   session created it.
2. A worker's current diff and target branch are visible before Integrate.
3. The coordinator receives an integration request without interrupting a
   running turn, and its outcome is easy to find.
4. An archived worker's branch and PR remain navigable; a checkout can be
   reconstructed when code needs editing again.
5. The complete loop works against the remote Loom host and Zed checkout.

The largest remaining bootstrap risk is operational: the remote Loom service
must be deployed and authenticated before the DGX walkthrough can run. Until
then, local and headless tests prove only bounded parts of the flow.

The full integration queue, deterministic fast path, and event vocabulary
guide later slices. They do not delay Phase 0: connect to personal Loom on the
DGX, run a real coding task, and open its actual remote checkout in Zed first.

## Current local conveniences

Project settings can save repository and base defaults on this device, scoped
to the selected Loom server. New tracks inherit these defaults when the draft
is empty; composed drafts keep their inputs. Repository suggestions combine
the connected server’s managed repositories, visible work, and recent choices.
The base picker accepts both discovered branches and typed refs.

Loom settings remember named server URLs and store each server’s token in
Keychain. Changing the URL clears the previous token before loading the new
server’s credential. Remote execution acceptance is still deferred.

The narrative acceptance scenarios and explicitly identified implementation
gaps are in [user-scenarios.md](user-scenarios.md). Passing a source audit or
local test does not establish deployed-app behavior; see the separate dogfood
record for what was actually exercised.
