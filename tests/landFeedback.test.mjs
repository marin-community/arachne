import assert from "node:assert/strict";
import test from "node:test";
import { landResultFeedback } from "../src/landFeedback.ts";

const landing = {
  landed: true,
  target_branch: "dev",
  commit: "abc123",
  commits_squashed: 2,
  primary_checkout: "/repo",
};

test("queued publishing, including local repair, reports intent without completion", () => {
  const note = landResultFeedback({ local: false });
  assert.equal(note, "Publish requested. Follow the actual result in this thread.");
  assert.doesNotMatch(note, /landed|published|already contained|nothing to do/i);
});

test("local=false cannot claim completion even with a landing payload", () => {
  const queued = landResultFeedback({ local: false });
  assert.equal(landResultFeedback({ local: false, landing }), queued);
  assert.equal(landResultFeedback({ local: false, landing: { ...landing, landed: false } }), queued);
});

test("completed local landing preserves commit, target and squash count", () => {
  assert.equal(landResultFeedback({ local: true, landing }),
    "Published locally: squash of 2 commits into dev · abc123");
  assert.equal(landResultFeedback({ local: true, landing: { ...landing, commits_squashed: 1 } }),
    "Published locally: squash of 1 commit into dev · abc123");
});

test("local no-op preserves actual already-contained feedback", () => {
  assert.equal(landResultFeedback({ local: true, landing: { ...landing, landed: false } }),
    "dev already contained this track's changes — nothing to do.");
});

test("missing local completion detail cannot claim a completed landing", () => {
  assert.equal(landResultFeedback({ local: true }), landResultFeedback({ local: false }));
});
