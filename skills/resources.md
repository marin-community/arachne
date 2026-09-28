# Resources skill

You are acting as a **worker or coordinator** inside a Loom topic. Arachne's
Resources panel shows the topic's resources: the repository, every thread's
pull requests, the GitHub issues the topic's threads work, and the checkout.
Two of those are **yours to keep current**: when you work on an issue, or when
you produce a PR, the topic's resource manifest should say so. Do it as part
of finishing the work, not as an afterthought.

The panel's live set (repo, PRs, issues, checkout) is derived automatically
from Loom's own data — you do **not** need to attach the topic's own repo,
checkout, or the PR Loom already tracks for your branch. What the manifest is
for is anything the derivation cannot see.

---

## When to record a binding

### 1. You start working on a GitHub issue

When your task is (or becomes) a GitHub issue — whether you found it, were
handed it, or opened it yourself — record it in Loom so the topic's issue set
is real:

```sh
loom issues add "Short title of the issue" --github 12
```

This creates a Loom work item **claimed by your branch**; the claim is what
makes the issue appear in the topic's Resources panel and in `loom issues ls`.
Use the GitHub issue number for `--github`. If the item is backlog (no branch
should own it yet), add `--repo`.

### 2. You open or take over a pull request

Loom's poll loop snapshots your branch's open PR automatically — the PR row
in the panel needs no action from you. But a PR that **outlives its checkout**
(or that the topic should remember even after archive) belongs in the durable
manifest. When you open a PR for the topic, also attach it:

Attach through Arachne's manifest with the `attach_topic_resource` command
(Arachne-side), or if you only have the Loom CLI, record the URL in your
report so the human can attach it. The binding to record:

```json
{
  "kind": "pull_request",
  "title": "Panel shows the full resource set",
  "repository": "<repo root>",
  "url": "https://github.com/OWNER/REPO/pull/13"
}
```

Link the PR back to your session so a reader on GitHub can find this thread:

```sh
gh pr create --body "$(printf 'Fixes #12\n\nloom: %s\n' "$(loom sessions url)")"
```

### 3. Same for issues the topic should remember durably

An issue you are *not* working but that the topic references (an upstream bug
this work fixes, a follow-up you are leaving for later) is attached, not
claimed:

```json
{
  "kind": "issue",
  "title": "Follow-up: recover checkout UX",
  "repository": "<repo root>",
  "url": "https://github.com/OWNER/REPO/issues/40"
}
```

---

## Rules

- **Identity is the URL.** A `pull_request`/`issue` binding's stable id is
  `pull_request:<url>` / `issue:<url>`. Re-attaching with a new title updates;
  it never duplicates.
- **Preserve the manifest's other fields.** The manifest may carry a
  `hidden` list (binding keys the topic opted out of inheriting from its
  project — design.md "Project defaults and resource inheritance"). Copy it
  through unchanged when you write a revision; entries are
  `<kind>:<repository>:<path>` for files and `<kind>:<url>` for PRs/issues,
  never resource ids.
- **Claim what you work.** `loom issues add` claims it for your branch; a
  durable manifest binding is for the topic's memory. Both are correct when
  both apply.
- **Never delete** another thread's binding; the manifest is shared and
  revision-checked. On a revision conflict, reload and retry once.
- **Do not attach** the topic's own repo, your checkout, or your live PR —
  those rows are derived; a manifest copy would only go stale.
- Titles are 1–256 characters; URLs must be on github.com.

## Why

The Resources panel is the topic's memory of what it touches: PRs and issues
appear there automatically when threads work them, and durable bindings keep
them navigable after sessions archive and checkouts vanish
(design.md: "an archived worker's branch and PR remain navigable").
