import assert from "node:assert/strict";
import test from "node:test";
import { attachedAsPanelPr, issueUrl, liveRows, mentionableRows, originLabel, prLight, prNumberFromUrl, repoLabel, repoUrl } from "../src/resourcePanel.ts";

const topic = (overrides = {}) => ({
  worktree_present: true,
  work_dir: "/w/topic",
  branch: { branch: "weaver/topic", repo_root: "/repos/acme/app" },
  github_repo: null,
  ...overrides,
});

const pr = (session_id, session_name, number, overrides = {}) => ({
  session_id,
  session_name,
  pr_number: number,
  pr_url: `https://github.com/acme/app/pull/${number}`,
  pr_state: "OPEN",
  pr_title: `PR ${number}`,
  is_draft: false,
  review_decision: null,
  checks: null,
  mergeable: null,
  ...overrides,
});

const issue = (id, overrides = {}) => ({
  id,
  repo_root: "/repos/acme/app",
  github_repo: "acme/app",
  github_issue: id,
  claimed_branch: "weaver/worker",
  source_branch: null,
  title: `Issue ${id}`,
  status: "open",
  ...overrides,
});

test("issue urls need both repo slug and github number", () => {
  assert.equal(issueUrl(issue(12)), "https://github.com/acme/app/issues/12");
  assert.equal(issueUrl(issue(12, { github_repo: null })), null);
  assert.equal(issueUrl(issue(12, { github_issue: null })), null);
  assert.equal(issueUrl(issue(0, { github_issue: 0 })), null);
});

test("repo label and url prefer the managed slug, else the repo root tail", () => {
  assert.equal(repoLabel(topic({ github_repo: "acme/app" })), "acme/app");
  assert.equal(repoLabel(topic()), "acme/app");
  assert.equal(repoUrl(topic({ github_repo: "acme/app" })), "https://github.com/acme/app");
  assert.equal(repoUrl(topic()), null);
});

test("live rows: repository, the topic's PR first then workers', open issues before closed", () => {
  const rows = liveRows(
    topic(),
    "topic-1",
    [pr("worker-a", "worker-a", 11), pr("topic-1", "coordinator", 10)],
    [issue(3), issue(1, { status: "closed" }), issue(2)],
  );
  assert.deepEqual(
    rows.map((row) => row.kind),
    ["repository", "pr", "pr", "issue", "issue", "issue"],
  );
  // The coordinator's PR first, labeled as the topic's own.
  assert.equal(rows[1].pr.pr_number, 10);
  assert.equal(rows[1].ownedByTopic, true);
  assert.equal(rows[2].pr.pr_number, 11);
  assert.equal(rows[2].ownedByTopic, false);
  // Open issues before closed, both by id.
  assert.deepEqual(
    rows.slice(3).map((row) => row.issue.id),
    [2, 3, 1],
  );
});

test("live rows cap PRs at 8 and issues at 12", () => {
  const prs = Array.from({ length: 10 }, (_, i) => pr(`s${i}`, `s${i}`, 100 + i));
  const issues = Array.from({ length: 15 }, (_, i) => issue(i + 1));
  const rows = liveRows(topic(), "topic-1", prs, issues);
  assert.equal(rows.filter((row) => row.kind === "pr").length, 8);
  assert.equal(rows.filter((row) => row.kind === "issue").length, 12);
});

test("pr light: green only when mergeable and CI passing; red on conflict or failure; yellow while pending or unknown", () => {
  // Green requires both known-good.
  assert.equal(prLight(pr("t", "t", 1, { checks: "passing", mergeable: "MERGEABLE" })), "green");
  assert.equal(prLight(pr("t", "t", 2, { checks: "passing", mergeable: null })), "yellow");
  assert.equal(prLight(pr("t", "t", 3, { checks: null, mergeable: "MERGEABLE" })), "yellow");
  // CI in progress is yellow even when mergeable.
  assert.equal(prLight(pr("t", "t", 4, { checks: "pending", mergeable: "MERGEABLE" })), "yellow");
  // Red: failing CI or a merge conflict, and either one wins over pending.
  assert.equal(prLight(pr("t", "t", 5, { checks: "failing", mergeable: "MERGEABLE" })), "red");
  assert.equal(prLight(pr("t", "t", 6, { checks: "pending", mergeable: "CONFLICTING" })), "red");
  // A closed or merged PR is not green — its state is over.
  assert.equal(prLight(pr("t", "t", 7, { pr_state: "MERGED", checks: "passing", mergeable: "MERGEABLE" })), "purple");
  assert.equal(prLight(pr("t", "t", 8, { pr_state: "CLOSED", checks: "passing", mergeable: "MERGEABLE" })), null);
});

test("live rows carry the status light", () => {
  const rows = liveRows(
    topic(),
    "topic-1",
    [pr("topic-1", "coordinator", 10, { checks: "passing", mergeable: "MERGEABLE" })],
    [],
  );
  assert.equal(rows[1].kind, "pr");
  assert.equal(rows[1].light, "green");
  // Merged shows purple; closed shows no light at all.
  const merged = liveRows(topic(), "topic-1", [pr("topic-1", "coordinator", 11, { pr_state: "MERGED" })], []);
  assert.equal(merged[1].light, "purple");
  const closed = liveRows(topic(), "topic-1", [pr("topic-1", "coordinator", 12, { pr_state: "CLOSED" })], []);
  assert.equal(closed[1].light, null);
});

test("an issue with no github link still renders, just without a url", () => {
  const local = issue(7, { github_repo: null, github_issue: null });
  const [row] = liveRows(topic(), "topic-1", [], [local]).slice(1);
  assert.equal(row.kind, "issue");
  assert.equal(row.url, null);
  assert.equal(row.key, "issue:7");
});

test("an attached PR URL + gh status renders through the same light as a live row", () => {
  // URL parse: number comes from the URL, not the manifest title.
  assert.equal(prNumberFromUrl("https://github.com/acme/app/pull/24"), 24);
  assert.equal(prNumberFromUrl("https://www.github.com/acme/app/pull/24/"), 24);
  assert.equal(prNumberFromUrl("https://gitlab.com/acme/app/pull/24"), null);
  assert.equal(prNumberFromUrl("https://github.com/acme/app/issues/24"), null);

  // Merged attachment → purple, closed → no light, exactly like live rows.
  const merged = attachedAsPanelPr("https://github.com/acme/app/pull/24", {
    state: "MERGED", mergeable: "UNKNOWN", checks: null, title: "Home: group rows",
  });
  assert.equal(prLight(merged), "purple");
  const closed = attachedAsPanelPr("https://github.com/acme/app/pull/27", {
    state: "CLOSED", mergeable: "CONFLICTING", checks: null, title: null,
  });
  assert.equal(prLight(closed), null);
  assert.equal(closed.pr_number, 27);
  assert.equal(closed.pr_title, "Pull request");
});

// --- Project resource inheritance (design.md "Project defaults and resource
//     inheritance") ----------------------------------------------------------

test("mentionable rows exclude hidden inherited bindings", () => {
  const rows = [
    { id: "a", kind: "file", title: "Attached", repository: "/r", reference: "b", path: "p", url: null, origin: "topic" },
    { id: "b", kind: "design_document", title: "Design", repository: "/r", reference: "b", path: "d", url: null, origin: "project" },
    { id: "c", kind: "issue", title: "Hidden", repository: "/r", reference: null, path: null, url: "https://github.com/a/b/issues/1", origin: "project", hidden: true },
  ];
  const mentionable = mentionableRows(rows);
  assert.deepEqual(mentionable.map((row) => row.id), ["a", "b"]);
});

test("origin labels distinguish project from topic", () => {
  assert.equal(originLabel("project"), "project");
  assert.equal(originLabel("topic"), "topic");
});

// --- Unified GitHub picker ---------------------------------------------------

test("kind of a picked GitHub search result comes from its URL", () => {
  const kindOf = (url) => (url.includes("/pull/") ? "pull_request" : "issue");
  assert.equal(kindOf("https://github.com/acme/app/pull/13"), "pull_request");
  assert.equal(kindOf("https://github.com/acme/app/issues/7"), "issue");
  // One search covers both kinds — nothing else distinguishes them.
});
