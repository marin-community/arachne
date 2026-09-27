# Arachne design

Status: bootstrap implementation, September 2026. This is the short, living
design document for the app. The broader intent and acceptance test are in
[handoff.md](handoff.md); integration details are in
[integration-and-landing.md](integration-and-landing.md).

## Purpose

Arachne is a macOS coding cockpit for long-running work. It helps one person
supervise topics and workers, notice decisions that need attention, inspect
their code, and move accepted work forward. Loom owns execution, durable
conversations, worktrees, artifacts, permissions, and event delivery. Arachne
should present those capabilities as a small set of understandable actions.

The bootstrap test is concrete: launch a coding worker, watch its conversation,
inspect its changes, open the same checkout in Zed, respond to attention, and
integrate its result into a topic. The Mac-local path is useful for development;
the intended remote test uses Loom on the DGX over Tailscale.

## User model

- **Topic:** the durable top-level object for intent, context, resources,
  subscriptions, mailbox, coordinator thread, workers, and integration state.
  Its coordinator may sleep. Accepted code state is a ref per attached
  repository, not one branch shared across repositories.
  Creation offers a short title and a body that becomes the agent goal; either
  can be left blank when the other or an attachment supplies the intent.
- **Quick topic:** a single-prompt way to start a topic. Its title can be
  derived from the prompt; title and body remain optional in the expanded
  creation form. It can grow delegated threads later without conversion.
- **Thread/worker:** one Loom session and its conversation. Hierarchy records
  responsibility; it does not determine Git ancestry.
- **Resource:** a repository, checkout, PR, document, file, or artifact with a
  stable identity independent of the session that first mentioned it.
- **Todo:** a durable, cross-topic action for the person. A topic's plan and a
  worker's internal checklist are separate from user Todos.
- **Attention:** a reason for the person to act. An idle worker is not an
  attention item; a blocked decision is.

The main screen answers “what needs me?” Topic and thread views then show the
conversation alongside code and resources. Every code reference should lead to
an editable checkout with one action when that checkout is available.

Every new top-level conversation is a topic, including one started through the
quick input. The Topics list expands through delegated threads at arbitrary
depth. Older unmarked top-level conversations remain visible as topics without
rewriting their Loom history. Delegating from a thread creates a child in the
conversation tree; its Git base still comes from the topic's accepted branch.

Worktrees are Resources, not Topics. A Topic may retain a checkout for human
editing, but its canonical accepted state is the repository ref. New coding
workers normally fork from that ref for their repository. This keeps
conversation hierarchy independent from Git ancestry.

## Resource slice

For bootstrap, attach a small manifest to the coordinator's Loom branch as a
versioned artifact. Each entry has a kind, title, repository, and backing
locator: a branch/ref and relative path for a file, or a URL for a PR. The
manifest points to existing objects; it does not copy a repository or document
into Arachne. Revision checks protect concurrent edits.

The first manual attachments are **design documents** and **files**. A coding
topic's design document is a normal file on its accepted branch, such as this
one. Arachne previews it through Loom's server-side worktree API and opens the
same file in Zed. The binding survives session archive, although preview and
editing require an active checkout. Loom artifacts remain appropriate for
agent-authored reports and standalone versioned documents.

Typing `@` in a thread message, a new topic body, or a quick task offers
attached resources.
The visible mention is readable, while Arachne resolves its stable ID against
the current manifest when sending and includes the backing locator in the
agent's prompt. Removing the visible mention removes that resource from the
send. New topics can cite resources from existing topics before they have
bindings of their own.

File attachments use Loom Scratch. At launch they seed the new session's
checkout; in an existing ACP thread, Arachne uploads them to that session and
passes Loom resource links with the prompt. The composer also accepts dropped
or pasted files. Scratch files are session inputs, while topic resource
bindings are durable references to repository files and artifacts.

Later resource types should be added only when a real workflow needs them.
The next likely additions are a retained integration checkout and a PR that
outlives its checkout. One active writer per mutable checkout remains the
default; other workers get isolated worktrees.

## Integration

The Topic owns integration candidates from its workers, with a queue and
status visible in the Topic view. **Integrate** absorbs a candidate into the
Topic's accepted ref for the relevant repository. **Land** moves that accepted
state to an upstream target such as a PR or main. A nested worker may integrate
through its parent, but worker → Topic is the normal visible path.

Worker lifecycle and integration lifecycle are separate. Stopping does not
make a coding result Ready. The worker normally commits or stabilizes its
changes, runs required validation, summarizes the result, and preflights it
against a specific Topic revision. Readiness records that revision (for
example, `Ready · clean against Arachne@789abc`); the worker can then sleep.
When the Topic ref advances, Loom should deterministically recheck sleeping
Ready candidates. Clean candidates get an updated readiness revision and stay
asleep. A new conflict emits an event
and preferentially wakes the original worker to reconcile.

Integrate is a structured operation: source, target, strategy, target revision,
validation policy, and result. Strategies include squash, merge, rebase,
cherry-pick, PR, and ask coordinator. A preflight conflict preview (for
example, `git merge-tree`) informs the UI before an attempt; the actual apply
and validation remain authoritative. For a clean,
unambiguous candidate, Loom can prepare a disposable worktree, apply the
strategy, validate, atomically advance the Topic ref, and clean up without an
LLM call. Failed attempts leave the canonical checkout and ref intact.
Conflicts, failing validation, ambiguity, and product judgment route to the
original worker, coordinator, or an integration worker as appropriate. A
request is not a success state; Arachne shows the verified commit or PR result.

## Events and attention

- **Event:** an immutable fact that something happened.
- **Source:** the emitter of Events.
- **Subscription:** a rule for which Events a Topic or Thread receives.
- **Mailbox:** durable pending Events and messages for a Topic or Thread.
- **Wake:** a decision to invoke an agent because of queued Events.

Timers, cron, GitHub webhooks, PR review and CI changes, W&B alerts, worker
completion, integration conflicts after a Topic ref changes, cross-thread
notes, runner reconnects, and manual messages all enter this model. Delivery
does not imply an LLM wake: Loom may coalesce Events, handle them
deterministically, or leave them queued until a later wake. This replaces busy
waiting without making every notification a model turn. Only unresolved human
decisions become Attention; integration or review decisions can create Todos.

Cross-thread knowledge transfer stays small: **Send to Thread…** can deliver
a note, selected message, resource or artifact, or generated summary with
provenance. A general worker-chat protocol is outside the bootstrap scope.

## Near-term acceptance

1. A topic can bind and preview this document without depending on which
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
