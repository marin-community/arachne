import assert from "node:assert/strict";
import test from "node:test";
import { blockKey, groupDisplayBlocks } from "../src/chatRows.ts";

const tool = (turn, seq) => ({
  kind: "tool_call",
  turn,
  seq,
  title: `tool ${seq}`,
});

test("groups calls across usage but respects visible boundaries and turns", () => {
  const rows = groupDisplayBlocks([
    tool(1, 1),
    { kind: "usage", turn: 1, seq: 2 },
    tool(1, 3),
    tool(1, 4),
    { kind: "thought", turn: 1, seq: 5 },
    tool(1, 6),
    { kind: "usage", turn: 1, seq: 7 },
    tool(2, 1),
  ]);
  assert.deepEqual(rows.map((row) => row.kind), [
    "tool_call_group", "single", "tool_call_group", "tool_call_group",
  ]);
  assert.deepEqual(rows.filter((row) => row.kind === "tool_call_group").map((row) => row.blocks.length), [3, 1, 1]);
});

test("turn_end splits groups even though it has no visual row", () => {
  const rows = groupDisplayBlocks([tool(1, 1), { kind: "turn_end", turn: 1, seq: 2 }, tool(2, 1)]);
  assert.deepEqual(rows.map((row) => row.kind), ["tool_call_group", "single", "tool_call_group"]);
});

test("journal keys survive older-page prepend and snapshot reload", () => {
  const tail = [tool(3, 20), tool(3, 21), { kind: "agent_message", turn: 3, seq: 22 }];
  const original = groupDisplayBlocks(tail);
  const withOlder = groupDisplayBlocks([{ kind: "user_message", turn: 2, seq: 1 }, ...tail]);
  assert.deepEqual(withOlder.slice(1).map((row) => row.key), original.map((row) => row.key));
  assert.deepEqual(groupDisplayBlocks([...tail]).map((row) => row.key), original.map((row) => row.key));
  assert.equal(blockKey(tail[0], 0), blockKey(tail[0], 100));
});

test("member keys identify a group after an older page joins its first call", () => {
  const original = groupDisplayBlocks([tool(4, 8), tool(4, 9)])[0];
  const extended = groupDisplayBlocks([tool(4, 7), tool(4, 8), tool(4, 9)])[0];
  assert.equal(original.kind, "tool_call_group");
  assert.equal(extended.kind, "tool_call_group");
  assert.notEqual(original.key, extended.key);
  assert.deepEqual(original.memberKeys, extended.memberKeys.slice(1));
});
