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

- **Topic:** durable intent and accepted state. Its coordinating thread may
  sleep, but the topic keeps its branch, documents, and resource bindings.
- **Thread/worker:** one Loom session and its conversation. Hierarchy records
  responsibility; it does not determine Git ancestry.
- **Resource:** a repository, checkout, PR, document, file, or artifact with a
  stable identity independent of the session that first mentioned it.
- **Attention:** a reason for the person to act. An idle worker is not an
  attention item; a blocked decision is.

The main screen answers “what needs me?” Topic and thread views then show the
conversation alongside code and resources. Every code reference should lead to
an editable checkout with one action when that checkout is available.

## Resource slice

For bootstrap, attach a small manifest to the topic's Loom branch as a
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

Later resource types should be added only when a real workflow needs them.
The next likely additions are a retained integration checkout and a PR that
outlives its checkout. One active writer per mutable checkout remains the
default; other workers get isolated worktrees.

## Integration

Workers normally fork from the topic's accepted branch. **Integrate** sends a
structured strategy request to the coordinator, which inspects the actual
source and target, handles conflicts, runs validation, and reports the commit
or PR. **Land** is a separate topic action toward upstream. A request is not a
success state. The app shows the actual diff before integration, and the
coordinator stamps the worker with the last verified commit or PR result.
The topic's conversation carries the detailed integration report.

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
