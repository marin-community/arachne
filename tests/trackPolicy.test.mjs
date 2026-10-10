import assert from "node:assert/strict";
import test from "node:test";
import { readFileSync } from "node:fs";
import { canonicalTopicBranch, resolveTrack, effectiveAutoland, trackAutoland, threadAutoland, inferTopicBranch, topicBranchError, isValidTopicBranch, threadLandFeedback, launchTopicBranch } from "../src/trackPolicy.ts";
import { independentReviewRequest, preparationRequest } from "../src/reviewRequest.ts";

const session = (id, parent = null, tags = [], repo = "/repo") => ({
  id, status: "running", parent_session_id: parent, parent_id: null,
  branch: { id: `branch-${id}`, branch: `weaver/${id}`, repo_root: repo, tags },
});
const tag = (key, value) => ({ key, value });

test("frontend matches the shared backend policy, ref, and inference contract fixtures", () => {
  const fixture = JSON.parse(readFileSync(new URL("./fixtures/track-policy-contract.json", import.meta.url), "utf8"));
  for (const { title, expected } of fixture.slug) assert.equal(inferTopicBranch(title), expected, title);
  for (const { value, valid } of fixture.refs) assert.equal(isValidTopicBranch(value), valid, value);
  for (const { track: trackPolicy, thread: threadPolicy, effective } of fixture.policy) {
    const track = session("track", null, trackPolicy ? [tag("track_autoland", trackPolicy)] : []);
    const source = session("source", track.id, threadPolicy ? [tag("thread_autoland", threadPolicy)] : []);
    assert.equal(effectiveAutoland(source, track) === "on", effective, `${trackPolicy}/${threadPolicy}`);
  }
});

test("canonical accepted ref differs from scratch; legacy absence preserves scratch", () => {
  const track = session("track", null, [tag("track_topic_branch", "arachne/tracks/task")]);
  assert.equal(canonicalTopicBranch(track), "arachne/tracks/task");
  assert.equal(canonicalTopicBranch(session("legacy")), "weaver/legacy");
});

test("untagged new Track resolves its durable initialization destination before legacy fallback", () => {
  const track = session("new-track");
  track.branch.goal = "Implement the task.\r\nArachne canonical Track initialization required: arachne/tracks/task\r\nInitialize from the launch seed before accepting changes.";
  assert.equal(canonicalTopicBranch(track), "arachne/tracks/task");
  for (const prompt of [independentReviewRequest("HEAD source", track), preparationRequest(false, "HEAD source", track)]) {
    assert.match(prompt, /arachne\/tracks\/task/);
    assert.doesNotMatch(prompt, /weaver\/new-track/);
  }
  // Once stamped, the durable canonical tag remains the primary authority.
  track.branch.tags.push(tag("track_topic_branch", "arachne/tracks/stamped"));
  assert.equal(canonicalTopicBranch(track), "arachne/tracks/stamped");
});

test("invalid or empty initialization markers never display a legacy scratch destination", () => {
  for (const required of ["", "HEAD", "refs/heads/topic", "bad ref"]) {
    const track = session("new-track");
    track.branch.goal = `Task\nArachne canonical Track initialization required: ${required}\n`;
    assert.equal(canonicalTopicBranch(track), "", required);
  }
  const legacy = session("legacy");
  legacy.branch.goal = "An ordinary task mentioning Arachne canonical Track initialization required: topic inline.";
  assert.equal(canonicalTopicBranch(legacy), "weaver/legacy");
});

test("Track resolution follows both parent identifiers, same-repository boundaries and cycle guards", () => {
  const root = session("root");
  const parent = session("parent", root.id);
  const worker = session("worker");
  worker.parent_id = parent.branch.id;
  assert.equal(resolveTrack([root, parent, worker], worker.id), root);
  assert.equal(resolveTrack([root], root.id), root);
  assert.equal(resolveTrack([session("foreign", null, [tag("topic", "true")], "/other"), session("w", "foreign")], "w"), null);
  assert.equal(resolveTrack([{ ...root, status: "archived" }, parent, worker], worker.id), null);
  assert.equal(resolveTrack([session("standalone", null, [tag("topic", "false")])], "standalone"), null);
  assert.equal(resolveTrack([session("a", "b"), session("b", "a")], "a"), null);
  assert.equal(resolveTrack([parent, worker], worker.id), null);
  const marked = session("marked", root.id, [tag("topic", "true")]);
  const nested = session("nested", marked.id);
  assert.equal(resolveTrack([root, marked, nested], nested.id), marked);
  const standalone = session("standalone", null, [tag("topic", "false")]);
  const intermediate = session("intermediate", standalone.id);
  const leaf = session("leaf", intermediate.id);
  assert.equal(resolveTrack([standalone, intermediate, leaf], leaf.id), null);
});

test("autoland defaults off/inherit; source overrides the Track, without inheriting intermediate source overrides", () => {
  for (const trackValue of [undefined, "bad", "off", "on"]) {
    const track = session("track", null, trackValue ? [tag("track_autoland", trackValue)] : []);
    for (const sourceValue of [undefined, "bad", "inherit", "off", "on"]) {
      const source = session("source", track.id, sourceValue ? [tag("thread_autoland", sourceValue)] : []);
      const expected = sourceValue === "on" || (sourceValue !== "off" && trackValue === "on") ? "on" : "off";
      assert.equal(effectiveAutoland(source, track), expected);
      assert.equal(effectiveAutoland(source, null), "off");
      assert.equal(effectiveAutoland({ ...source, branch: { ...source.branch, repo_root: "/other" } }, track), "off");
    }
    assert.equal(trackAutoland(track), trackValue === "on" ? "on" : "off");
    assert.equal(threadAutoland(session("worker")), "inherit");
  }
});

test("branch inference prefers explicit title, then task; empty field stays backend inference", () => {
  assert.equal(inferTopicBranch(" Fix the UI! ", "Other task"), "arachne/tracks/fix-the-ui");
  assert.equal(inferTopicBranch("", "Land / Track: revised UI"), "arachne/tracks/land-track-revised-ui");
  assert.equal(inferTopicBranch("", ""), "arachne/tracks/track");
  assert.equal(inferTopicBranch("K İ 日本語"), "arachne/tracks/track");
  assert.equal(topicBranchError(""), null);
});

test("explicit ref validation rejects Git ref expressions and reserved forms", () => {
  for (const ref of ["HEAD", "@", "refs/heads/task", "-task", "/task", "task/", "task//child", ".hidden", "a/.b", "task.lock", "a/task.lock/b", "a..b", "a@{1}", "a b", "a\nb", "a~1", "a^", "a:b", "a?b", "a*b", "a[b", "a\\b", "task."]) {
    assert.ok(topicBranchError(ref), ref);
  }
  for (const ref of ["arachne/tracks/task", "feature/task-v2", "fix_UI", "foo@bar", "single", "weaver/custom", "café/task"]) assert.equal(topicBranchError(ref), null, ref);
});

test("manual Land reports destination, delivery warnings and pending acceptance", () => {
  const note = threadLandFeedback({ coordinator_id: "track", coordinator_name: "Track", target_branch: "arachne/tracks/task", warnings: ["Delivery delayed."] });
  assert.match(note, /Land requested.*arachne\/tracks\/task/);
  assert.match(note, /Waiting for successful wrap-up and acceptance/);
  assert.match(note, /Delivery delayed/);
  assert.doesNotMatch(note, /landed|published/i);
});


test("launch metadata sends explicit refs verbatim and lets backend infer/disambiguate empty Tracks", () => {
  assert.equal(launchTopicBranch("feature/accepted", true), "feature/accepted");
  assert.equal(launchTopicBranch("", true), null);
  assert.equal(launchTopicBranch(undefined, true), null);
  assert.equal(launchTopicBranch("feature/accepted", false), null);
  assert.throws(() => launchTopicBranch("HEAD", true), /local branch/);
  assert.throws(() => launchTopicBranch(" ", true), /valid local Git branch/);
});
