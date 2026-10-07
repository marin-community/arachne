import assert from "node:assert/strict";
import test from "node:test";
import { announceNotices, osNotificationsEnabled, OS_NOTIFICATIONS_SETTING } from "../src/osNotifications.ts";

// A structural slice of AttentionNotice: only the fields announceNotices reads.
interface NoticeSlice { key: string; title: string; reason: string; level: "attention" | "blocked" }

const notice = (key: string, extra: Partial<NoticeSlice> = {}): NoticeSlice => ({
  key, sessionId: `s-${key}`, topicId: `s-${key}`, title: "topic · worker",
  reason: "Choose a release target", action: "Resolve blocker", level: "blocked", ...extra,
});

test("announceNotices sends once per key and again when the key changes", () => {
  const sent: [string, string][] = [];
  const send = (title: string, body: string) => sent.push([title, body]);
  const first = announceNotices([notice("a")], new Set(), send);
  assert.deepEqual(sent, [["Needs you — blocked", "topic · worker: Choose a release target"]]);
  // Same identity again (SSE snapshot, reconnect): no repeat.
  announceNotices([notice("a")], first, send);
  assert.equal(sent.length, 1);
  // A new request/note/tag combination is a new identity: notify again.
  announceNotices([notice("b")], first, send);
  assert.deepEqual(sent[1], ["Needs you — blocked", "topic · worker: Choose a release target"]);
  // Attention level shapes the title.
  const second = announceNotices([notice("c", { level: "attention" })], first, send);
  assert.equal(sent[2][0], "Needs you");
  // A resolved notice drops out of the shown set, so a recurrence resurfaces.
  assert.equal(announceNotices([], second, send).size, 0);
  announceNotices([notice("c", { level: "attention" })], announceNotices([], second, send), send);
  assert.equal(sent.length, 4);
});

test("the OS notification setting defaults on and persists", () => {
  const data = new Map<string, string>();
  const storage = { getItem: (key: string) => data.get(key) ?? null, setItem: (key: string, value: string) => { data.set(key, value); } } as Storage;
  assert.equal(osNotificationsEnabled(storage), true);
  storage.setItem(OS_NOTIFICATIONS_SETTING, "false");
  assert.equal(osNotificationsEnabled(storage), false);
  storage.setItem(OS_NOTIFICATIONS_SETTING, "true");
  assert.equal(osNotificationsEnabled(storage), true);
});
