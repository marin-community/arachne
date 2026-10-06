import assert from "node:assert/strict";
import test from "node:test";
import {
  blockKey,
  commandReplyTurns,
  blockCopyText,
  copyCornerFor,
  countThinkingTokens,
  formatPlanEntries,
  formatTokens,
  groupDisplayBlocks,
  resolveContextUsage,
  toolCallCopyText,
} from "../src/chatRows.ts";

const tool = (turn, seq) => ({
  kind: "tool_call",
  turn,
  seq,
  title: `tool ${seq}`,
});

const thought = (turn, seq, text) => ({
  kind: "thought",
  turn,
  seq,
  text,
  summary: text.split("\n")[0],
});

test("groups calls across usage but respects visible boundaries and turns", () => {
  const rows = groupDisplayBlocks([
    tool(1, 1),
    { kind: "usage", turn: 1, seq: 2 },
    tool(1, 3),
    tool(1, 4),
    { kind: "agent_message", turn: 1, seq: 5 },
    tool(1, 6),
    { kind: "usage", turn: 1, seq: 7 },
    tool(2, 1),
  ]);
  assert.deepEqual(
    rows.map((row) => row.kind),
    ["work_group", "single", "work_group", "work_group"],
  );
  assert.deepEqual(
    rows.filter((row) => row.kind === "work_group").map((row) => row.blocks.length),
    [3, 1, 1],
  );
});

test("groups thoughts together with tool calls in the same turn", () => {
  const rows = groupDisplayBlocks([
    thought(1, 1, "**Planning**\nwhat next"),
    { kind: "usage", turn: 1, seq: 2 },
    tool(1, 3),
    thought(1, 4, "reflect"),
    { kind: "agent_message", turn: 1, seq: 5 },
    thought(1, 6, "after the reply"),
  ]);
  const groups = rows.filter((row) => row.kind === "work_group");
  assert.equal(groups.length, 2);
  assert.equal(groups[0].blocks.length, 3);
  assert.equal(groups[0].calls.length, 1);
  assert.equal(groups[0].thoughts.length, 2);
  assert.equal(groups[1].blocks.length, 1);
  // A thought standing alone still groups (with zero calls), so it collapses.
  assert.deepEqual(
    rows.map((row) => row.kind),
    ["work_group", "single", "work_group"],
  );
});

test("joins old journal prose interrupted by a thought delta", () => {
  const blocks = [
    thought(5, 127, "reasoning"),
    { kind: "agent_message", turn: 5, seq: 128, text: "Two" },
    thought(5, 129, "."),
    { kind: "agent_message", turn: 5, seq: 130, text: " distinct problems found." },
    tool(5, 131),
    { kind: "agent_message", turn: 5, seq: 132, text: "Next message" },
  ];
  const rows = groupDisplayBlocks(blocks, null, true);
  assert.deepEqual(rows.map((row) => row.kind), ["work_group", "single", "work_group", "single"]);
  assert.equal(rows[0].thoughts.length, 2);
  assert.equal(rows[1].block.text, "Two distinct problems found.");
  assert.equal(rows[1].key, "block:5:130");
  assert.equal(rows[3].block.text, "Next message");
  assert.equal(groupDisplayBlocks(blocks).filter((row) => row.kind === "single").length, 3);
});

test("does not join prose across turns or a tool boundary", () => {
  const rows = groupDisplayBlocks([
    { kind: "agent_message", turn: 1, seq: 1, text: "First" },
    thought(1, 2, "thinking"),
    tool(1, 3),
    { kind: "agent_message", turn: 1, seq: 4, text: "Second" },
    thought(2, 1, "new turn"),
    { kind: "agent_message", turn: 2, seq: 2, text: "Third" },
  ], null, true);
  assert.deepEqual(
    rows.filter((row) => row.kind === "single").map((row) => row.block.text),
    ["First", "Second", "Third"],
  );
});

test("the live turn's trailing thought stays its own open row", () => {
  const blocks = [
    thought(2, 1, "earlier thinking"),
    tool(2, 2),
    thought(2, 3, "currently thinking"),
  ];
  const rows = groupDisplayBlocks(blocks, 2);
  assert.deepEqual(
    rows.map((row) => row.kind),
    ["work_group", "live_thought"],
  );
  const group = rows[0];
  assert.equal(group.kind, "work_group");
  assert.equal(group.state, "thinking");
  assert.equal(group.blocks.length, 2);
  assert.equal(rows[1].kind, "live_thought");
  assert.equal(rows[1].block.text, "currently thinking");
  // Once the turn ends, all three collapse into one group.
  const done = groupDisplayBlocks(blocks, null);
  assert.deepEqual(
    done.map((row) => row.kind),
    ["work_group"],
  );
  assert.equal(done[0].state, "done");
});

test("a trailing thought that is not last does not hold a live row", () => {
  const blocks = [thought(2, 1, "thought"), tool(2, 2)];
  const rows = groupDisplayBlocks(blocks, 2);
  // The trailing block is a tool call, not a thought: no live-thought row.
  assert.deepEqual(
    rows.map((row) => row.kind),
    ["work_group"],
  );
  assert.equal(rows[0].state, "thinking");
});

test("turn_end splits groups even though it has no visual row", () => {
  const rows = groupDisplayBlocks([tool(1, 1), { kind: "turn_end", turn: 1, seq: 2 }, tool(2, 1)]);
  assert.deepEqual(
    rows.map((row) => row.kind),
    ["work_group", "single", "work_group"],
  );
});

test("journal keys survive older-page prepend and snapshot reload", () => {
  const tail = [tool(3, 20), tool(3, 21), { kind: "agent_message", turn: 3, seq: 22 }];
  const original = groupDisplayBlocks(tail);
  const withOlder = groupDisplayBlocks([{ kind: "user_message", turn: 2, seq: 1 }, ...tail]);
  assert.deepEqual(
    withOlder.slice(1).map((row) => row.key),
    original.map((row) => row.key),
  );
  assert.deepEqual(
    groupDisplayBlocks([...tail]).map((row) => row.key),
    original.map((row) => row.key),
  );
  assert.equal(blockKey(tail[0], 0), blockKey(tail[0], 100));
});

test("member keys identify a group after an older page joins its first call", () => {
  const original = groupDisplayBlocks([tool(4, 8), tool(4, 9)])[0];
  const extended = groupDisplayBlocks([tool(4, 7), tool(4, 8), tool(4, 9)])[0];
  assert.equal(original.kind, "work_group");
  assert.equal(extended.kind, "work_group");
  assert.notEqual(original.key, extended.key);
  assert.deepEqual(original.memberKeys, extended.memberKeys.slice(1));
});

test("copy text returns the message text, trimmed", () => {
  assert.equal(blockCopyText({ kind: "user_message", text: "  hi there \n" }), "hi there");
  assert.equal(blockCopyText({ kind: "agent_message", text: "done!" }), "done!");
  assert.equal(blockCopyText({ kind: "agent_message" }), "");
});

test("copy text formats plan entries one per line with status prefixes", () => {
  assert.equal(formatPlanEntries([["Write tests", "pending"], ["Ship", "completed"]]), "[pending] Write tests\n[completed] Ship");
  const text = blockCopyText({ kind: "plan", entries: [["Step", "in_progress"]] });
  assert.equal(text, "[in_progress] Step");
});

test("copy text formats tool call content parts and skips images", () => {
  const call = {
    kind: "tool_call",
    turn: 1,
    seq: 1,
    title: "Read files",
    content: [
      { type: "text", text: "a.txt" },
      { type: "image", data: "aGVsbG8=", mime_type: "image/png" },
      { type: "diff", path: "b.txt" },
    ],
  };
  assert.equal(toolCallCopyText(call), "Read files\na.txt\ndiff b.txt");
});

test("copy text for thought blocks preserves the raw text untrimmed", () => {
  assert.equal(blockCopyText({ kind: "thought", text: "\n\n**Thinking**\nstep 1" }), "\n\n**Thinking**\nstep 1");
});

test("copy corner follows the visible edge: top while the host's top is on screen, bottom once scrolled past", () => {
  // viewport top at 0: a host starting at or below it shows its top corner.
  assert.equal(copyCornerFor(0, 0), "top");
  assert.equal(copyCornerFor(50, 0), "top");
  // a host whose top has scrolled above the viewport pins bottom.
  assert.equal(copyCornerFor(-400, 0), "bottom");
  // scrolled viewport: same rule against its own top edge.
  assert.equal(copyCornerFor(-100, -300), "top");
  assert.equal(copyCornerFor(-350, -300), "bottom");
});

test("thinking token totals count only the group's own thoughts", () => {
  // A usage block is a cumulative context gauge for the whole trace, so
  // it must not leak into the group's thinking total.
  const blocks = [
    thought(1, 1, "a".repeat(40)),
    { kind: "usage", turn: 1, seq: 2, used: 45_000, size: 200_000 },
    thought(1, 3, "b".repeat(400)),
  ];
  const rows = groupDisplayBlocks(blocks);
  const group = rows[0];
  assert.equal(group.kind, "work_group");
  assert.equal(group.thinkingTokens, 110);
  // Directly: the estimate is chars/4 over exactly the given thoughts.
  assert.equal(countThinkingTokens([blocks[0], blocks[2]]), 110);
});

test("context usage prefers the newest journal report over a stale zero summary", () => {
  const summary = { used: 0, size: 131_072, cost: { amount: 1.25, currency: "USD" } };
  const usage = resolveContextUsage([
    { kind: "usage", turn: 1, seq: 4, used: 18_000, size: 131_072 },
    { kind: "agent_message", turn: 1, seq: 5, text: "done" },
    { kind: "usage", turn: 2, seq: 7, used: 30_000, size: 131_072 },
  ], summary);
  assert.deepEqual(usage, {
    used: 30_000,
    size: 131_072,
    cost: { amount: 1.25, currency: "USD" },
  });
});

test("context usage falls back to the session summary for old or malformed journals", () => {
  const summary = { used: 12_000, size: 64_000, cost: null };
  assert.deepEqual(resolveContextUsage([], summary), summary);
  assert.deepEqual(resolveContextUsage([
    { kind: "usage", turn: 0, seq: 0 },
    { kind: "usage", turn: 1, seq: 0, used: -1, size: 0 },
  ], summary), summary);
});

test("formatTokens renders K and M human-friendly", () => {
  assert.equal(formatTokens(0), "0");
  assert.equal(formatTokens(999), "999");
  assert.equal(formatTokens(1000), "1K");
  assert.equal(formatTokens(1500), "1.5K");
  assert.equal(formatTokens(12_500), "13K");
  assert.equal(formatTokens(125_000), "125K");
  assert.equal(formatTokens(1_000_000), "1M");
  assert.equal(formatTokens(3_200_000), "3.2M");
});

test("work group summary counts calls and thinking tokens", () => {
  const rows = groupDisplayBlocks([
    thought(1, 1, "thinking hard about it"),
    tool(1, 2),
    tool(1, 3),
  ]);
  const group = rows[0];
  assert.equal(group.kind, "work_group");
  assert.equal(group.calls.length, 2);
  assert.equal(group.state, "done");
});

// -- commandReplyTurns -------------------------------------------------------

test("commandReplyTurns marks interceptor-answered slash command turns", () => {
  const commands = ["compact", "session", "export"];
  // /session: user command → interceptor reply, no model work. A usage
  // block inside the turn would mean the model ran — excluded by shape.
  const rows = commandReplyTurns([
    { kind: "user_message", turn: 1, seq: 0, text: "/session" },
    { kind: "agent_message", turn: 1, seq: 1, text: "Session: abc" },
    { kind: "turn_end", turn: 1, seq: 2, stop_reason: "end_turn" },
  ], commands);
  assert.deepEqual([...rows.entries()], [[1, "session"]]);
});

test("commandReplyTurns accepts commands with args (pi-acp splits on the first space)", () => {
  const rows = commandReplyTurns([
    { kind: "user_message", turn: 1, seq: 0, text: "/compact keep the test context" },
    { kind: "agent_message", turn: 1, seq: 1, text: "Compaction completed." },
  ], ["compact"]);
  assert.deepEqual([...rows.entries()], [[1, "compact"]]);
});

test("commandReplyTurns drops turns with model work — usage, thoughts, or tool calls", () => {
  // File commands (.pi/commands/*.md) expand into prompts the model
  // answers; usage reports appear on every model turn.
  assert.equal(commandReplyTurns([
    { kind: "user_message", turn: 1, seq: 0, text: "/review" },
    { kind: "thought", turn: 1, seq: 1, text: "running the review" },
    { kind: "tool_call", turn: 1, seq: 2, title: "run" },
    { kind: "agent_message", turn: 1, seq: 3, text: "review output" },
  ], ["review"]).size, 0);
  // A pure-prose model turn still journals usage — codex always reports it.
  assert.equal(commandReplyTurns([
    { kind: "user_message", turn: 1, seq: 0, text: "/status" },
    { kind: "usage", turn: 1, seq: 1, used: 100, size: 200 },
    { kind: "agent_message", turn: 1, seq: 2, text: "status output" },
    { kind: "turn_end", turn: 1, seq: 3, stop_reason: "end_turn" },
  ], ["status"]).size, 0);
});

test("commandReplyTurns ignores unknown commands and plain prompts", () => {
  assert.equal(commandReplyTurns([
    { kind: "user_message", turn: 1, seq: 0, text: "/not-a-command" },
    { kind: "agent_message", turn: 1, seq: 1, text: "hi" },
  ], ["session"]).size, 0);
  assert.equal(commandReplyTurns([
    { kind: "user_message", turn: 1, seq: 0, text: "explain /compact" },
    { kind: "agent_message", turn: 1, seq: 1, text: "sure" },
  ], ["compact"]).size, 0);
  assert.equal(commandReplyTurns([], ["compact"]).size, 0);
  assert.equal(commandReplyTurns([
    { kind: "user_message", turn: 1, seq: 0, text: "/compact" },
  ], []).size, 0);
});

test("commandReplyTurns only the turn's opening message counts", () => {
  // A steering message sent mid-turn carries /command text but is model-
  // facing prose journaled as its own user_message; only the message that
  // opens the turn can name the interceptor command.
  const rows = commandReplyTurns([
    { kind: "user_message", turn: 1, seq: 0, text: "please compact the context" },
    { kind: "agent_message", turn: 1, seq: 1, text: "ok" },
    { kind: "user_message", turn: 2, seq: 0, text: "/session" },
    { kind: "agent_message", turn: 2, seq: 1, text: "Session: x" },
  ], ["session"]);
  assert.deepEqual([...rows.entries()], [[2, "session"]]);
});
