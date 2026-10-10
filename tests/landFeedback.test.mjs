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


test("completed publish remains completion if a follow-up commit revision is unavailable", () => {
  const feedback = landResultFeedback({ local: true, landing: {
    landed: true, target_branch: "primary", commits_squashed: 2, primary_checkout: "/repo",
  } });
  assert.match(feedback, /Published locally.*primary/);
  assert.match(feedback, /commit revision unavailable/);
  assert.doesNotMatch(feedback, /undefined|requested|failed/);
});


test("post-completion warnings do not turn a successful Git write into a queued or failed operation", () => {
  const feedback = landResultFeedback({ local: true, landing: { ...landing,
    warnings: ["Git completed; revision metadata could not be refreshed."],
  } });
  assert.match(feedback, /Published locally.*abc123/);
  assert.match(feedback, /revision metadata could not be refreshed/);
  assert.doesNotMatch(feedback, /Publish requested/);
});
