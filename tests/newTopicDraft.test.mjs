import assert from "node:assert/strict";
import test from "node:test";
import {
  NEW_TOPIC_DRAFT_KEY,
  draftStorageKey,
  initializeDraftStorage,
  clearDraft,
  draftHasContent,
  emptyDraft,
  mergeLaunchConfig,
  readDraft,
  saveDraft,
} from "../src/newTopicDraft.ts";

const store = () => {
  let kv = new Map();
  return {
    getItem: (k) => kv.get(k) ?? null,
    setItem: (k, v) => kv.set(k, v),
    removeItem: (k) => kv.delete(k),
  };
};

const DRAFT_STORAGE_KEY = draftStorageKey(store());

test("emptyDraft has the launch-sheet defaults", () => {
  assert.deepEqual(emptyDraft(), {
    title: "", body: "", repo: "", base: "",
    profile: "default", agent: "", model: "", effort: "",
    mentions: [], attachments: [],
  });
});

test("readDraft tolerates absent/garbage/partial payloads field by field", () => {
  const s = store();
  assert.deepEqual(readDraft(s), emptyDraft());
  s.setItem(DRAFT_STORAGE_KEY, "not json");
  assert.deepEqual(readDraft(s), emptyDraft());
  s.setItem(DRAFT_STORAGE_KEY, JSON.stringify("weird"));
  assert.deepEqual(readDraft(s), emptyDraft());
  s.setItem(DRAFT_STORAGE_KEY, JSON.stringify(["array"]));
  assert.deepEqual(readDraft(s), emptyDraft());
  // A stale payload from an older build: unknown fields are ignored, and
  // the known-but-absent ones fall back to the sheet's own defaults.
  s.setItem(DRAFT_STORAGE_KEY, JSON.stringify({ extra: "dropped", title: "kept" }));
  assert.deepEqual(readDraft(s), { ...emptyDraft(), title: "kept" });
});

test("saveDraft + readDraft round-trip (ignoring transient fields)", () => {
  const s = store();
  const draft = {
    title: "Ship the sheet",
    body: "Fix the flow\n\n@{design doc}",
    repo: "marin-community/arachne",
    base: "weaver/my-branch",
    profile: "default",
    agent: "codex",
    model: "gpt-5.2",
    effort: "high",
    mentions: [{ token: "@{design doc}", topicId: "t1", resourceId: "r1" }],
    attachments: [{ name: "f.txt", size: 3, contentBase64: "aGk=", mimeType: "text/plain" }],
  };
  saveDraft(s, draft);
  assert.deepEqual(readDraft(s), draft);
  // A saved draft outlives the sheet that produced it (fields added after
  // the fact are dropped by the merge, not resurrected).
  s.setItem(DRAFT_STORAGE_KEY, JSON.stringify({ ...draft, extra: "dropped" }));
  assert.deepEqual(readDraft(s), draft);
});

test("readDraft accepts string scalars and drops nulls", () => {
  const s = store();
  s.setItem(DRAFT_STORAGE_KEY, JSON.stringify({
    title: "t", body: null, repo: null, base: 42,
    profile: null, agent: null, model: null, effort: null,
    mentions: null, attachments: null,
  }));
  assert.deepEqual(readDraft(s), {
    title: "t", body: "", repo: "", base: "",
    profile: "default", agent: "", model: "", effort: "",
    mentions: [], attachments: [],
  });
});

test("readDraft keeps only attachment fields the launch path needs", () => {
  const s = store();
  const attachment = { name: "a.png", size: 10, contentBase64: "AAA", mimeType: "image/png" };
  saveDraft(s, {
    ...emptyDraft(),
    attachments: [{ ...attachment, previewUrl: "blob:x", extra: 1 }],
  });
  const { attachments } = readDraft(s);
  assert.deepEqual(attachments, [attachment]);
});

test("mergeLaunchConfig selects the default profile and keeps unrelated selects", () => {
  const s = store();
  saveDraft(s, {
    ...emptyDraft(),
    profile: "codex",
    agent: "codex",
    model: "gpt-5.2",
    effort: "high",
  });
  const options = { profiles: [
    { name: "default", class: "interactive", agent_kind: "pi", model: "", effort: "" },
    { name: "claude", class: "interactive", agent_kind: "claude-code", model: "", effort: "" },
  ], agents: [{ kind: "pi" }, { kind: "codex" }], default_agent: "pi" };
  assert.deepEqual(mergeLaunchConfig(readDraft(s), options), {
    profile: "default", agent: "codex", model: "gpt-5.2", effort: "high",
  });
  // No options at all (still loading / loom unreachable): nothing is
  // reconciled — the typed profile stays rather than silently relaunching
  // under the default route.
  assert.deepEqual(mergeLaunchConfig(readDraft(s), null), {
    profile: "codex", agent: "codex", model: "gpt-5.2", effort: "high",
  });
  // Profiles present but the draft's is gone: fall back to the last
  // profile that still exists; an agent that no longer exists resets to
  // the runtime default.
  assert.deepEqual(mergeLaunchConfig(readDraft(s), { profiles: [], agents: [] }), {
    profile: "default", agent: "", model: "gpt-5.2", effort: "high",
  });
  // Options still loading (no agent list): a typed agent survives, and a
  // stored agent that no longer exists resets to the runtime default once
  // the list arrives.
  assert.equal(mergeLaunchConfig(readDraft(s), { profiles: options.profiles }).agent, "codex");
  assert.equal(mergeLaunchConfig({ ...emptyDraft(), profile: "gone", agent: "gone" }, options).agent, "");
});

test("draftHasContent: any non-empty field counts", () => {
  assert.equal(draftHasContent(emptyDraft()), false);
  assert.equal(draftHasContent({ ...emptyDraft(), title: " t " }), true);
  assert.equal(draftHasContent({ ...emptyDraft(), body: " b " }), true);
  assert.equal(draftHasContent({ ...emptyDraft(), attachments: [{ name: "x" }] }), true);
});

test("clearDraft wipes the stored payload", () => {
  const s = store();
  saveDraft(s, { ...emptyDraft(), title: "x" });
  clearDraft(s);
  assert.deepEqual(readDraft(s), emptyDraft());
});


test("drafts stay scoped to their server and legacy migration belongs to startup server", () => {
  const s = store();
  s.setItem("loomUrl", "http://first:7878");
  s.setItem(NEW_TOPIC_DRAFT_KEY, JSON.stringify({ ...emptyDraft(), body: "legacy work", mentions: [{ token: "@{doc}", topicId: "first-track", resourceId: "first-resource" }] }));
  initializeDraftStorage(s);
  assert.equal(readDraft(s).body, "legacy work");
  assert.equal(s.getItem(NEW_TOPIC_DRAFT_KEY), null);
  s.setItem("loomUrl", "http://second:7878");
  assert.deepEqual(readDraft(s), emptyDraft());
  saveDraft(s, { ...emptyDraft(), body: "second work" });
  clearDraft(s);
  s.setItem("loomUrl", "http://first:7878/");
  assert.equal(readDraft(s).body, "legacy work");
  assert.equal(readDraft(s).mentions[0].resourceId, "first-resource");
});
