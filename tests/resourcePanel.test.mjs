import assert from "node:assert/strict";
import test from "node:test";
import { issueUrl, liveRows, repoLabel, repoUrl } from "../src/resourcePanel.ts";

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

test("an issue with no github link still renders, just without a url", () => {
  const local = issue(7, { github_repo: null, github_issue: null });
  const [row] = liveRows(topic(), "topic-1", [], [local]).slice(1);
  assert.equal(row.kind, "issue");
  assert.equal(row.url, null);
  assert.equal(row.key, "issue:7");
});
