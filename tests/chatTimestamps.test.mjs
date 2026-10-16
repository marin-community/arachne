import assert from "node:assert/strict";
import test from "node:test";
import { groupDisplayBlocks } from "../src/chatRows.ts";
import { formatChatTimestamp, withChatTimestamps } from "../src/chatTimestamps.ts";

const message = (seq, created_at) => ({ kind: "user_message", turn: 1, seq, created_at });
const separators = (blocks) => withChatTimestamps(groupDisplayBlocks(blocks));

test("labels the first row and gaps of at least five minutes between activities", () => {
  const rows = separators([
    message(0, "2026-10-14T12:00:00Z"),
    message(1, "2026-10-14T12:04:59Z"),
    message(2, "2026-10-14T12:09:59Z"),
  ]);
  assert.deepEqual(rows.map((row) => row.timestamp), [
    "2026-10-14T12:00:00.000Z", null, "2026-10-14T12:09:59.000Z",
  ]);
});

test("a new local calendar day gets a label even one minute later", () => {
  const before = new Date(2026, 9, 14, 23, 59);
  const after = new Date(2026, 9, 15, 0, 0);
  assert.equal(separators([
    message(0, before.toISOString()), message(1, after.toISOString()),
  ])[1].timestamp, after.toISOString());
});

test("missing and invalid journal times are skipped without inventing dates", () => {
  const rows = separators([
    message(0, null), message(1, "invalid"), message(2, "2026-10-14T12:00:00Z"),
    message(3, undefined), message(4, "2026-10-14T12:01:00Z"),
  ]);
  assert.deepEqual(rows.map((row) => row.timestamp), [
    null, null, "2026-10-14T12:00:00.000Z", null, null,
  ]);
});

test("collapsed work groups use their latest member as activity", () => {
  const rows = separators([
    message(0, "2026-10-14T12:00:00Z"),
    { kind: "thought", turn: 1, seq: 1, created_at: "2026-10-14T12:01:00Z" },
    { kind: "tool_call", turn: 1, seq: 2, created_at: "2026-10-14T12:20:00Z" },
    message(3, "2026-10-14T12:21:00Z"),
    message(4, "2026-10-14T12:26:00Z"),
  ]);
  assert.deepEqual(rows.map((row) => row.timestamp), [
    "2026-10-14T12:00:00.000Z", null, null, "2026-10-14T12:26:00.000Z",
  ]);
});

test("older-page prepends recompute boundaries while retaining journal keys", () => {
  const tail = [message(2, "2026-10-14T12:02:00Z"), message(3, "2026-10-14T12:08:00Z")];
  const original = separators(tail);
  const extended = separators([message(1, "2026-10-14T12:01:00Z"), ...tail]);
  assert.equal(extended[1].timestamp, null);
  assert.deepEqual(extended.slice(1).map((row) => row.key), original.map((row) => row.key));
  assert.equal(extended[2].timestamp, original[1].timestamp);
});

test("out-of-order records do not move the activity clock backwards", () => {
  const rows = separators([
    message(0, "2026-10-14T12:10:00Z"), message(1, "2026-10-14T12:01:00Z"),
    message(2, "2026-10-14T12:11:00Z"),
  ]);
  assert.equal(rows[2].timestamp, null);
});

test("formats local Today, Yesterday, older dates and older years", () => {
  const now = new Date(2026, 9, 15, 12);
  const format = (date, locale = "en-US") => formatChatTimestamp(date.toISOString(), now, locale);
  assert.equal(format(new Date(2026, 9, 15, 9, 5)), "Today, 9:05 AM");
  assert.equal(format(new Date(2026, 9, 14, 23, 30)), "Yesterday, 11:30 PM");
  assert.equal(format(new Date(2026, 9, 13, 9, 5)), "Tue, Oct 13, 9:05 AM");
  assert.equal(format(new Date(2025, 9, 13, 9, 5)), "Mon, Oct 13, 2025, 9:05 AM");
  assert.equal(format(new Date(2026, 9, 15, 9, 5), "en-GB"), "Today, 9:05");
  assert.equal(formatChatTimestamp("invalid", now), "");
});

test("Yesterday uses the calendar day across DST and year boundaries", () => {
  // With TZ=America/Los_Angeles this pair straddles the fall-back day.
  assert.match(formatChatTimestamp(new Date(2026, 9, 31, 0, 30).toISOString(),
    new Date(2026, 10, 1, 23, 30), "en-US"), /^Yesterday, /);
  assert.match(formatChatTimestamp(new Date(2025, 11, 31, 23, 30).toISOString(),
    new Date(2026, 0, 1, 0, 30), "en-US"), /^Yesterday, /);
});
