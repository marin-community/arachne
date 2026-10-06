/** A chat block with its journal position, supplied by fetch_chat. */
export interface ChatDisplayBlock {
  kind: string;
  turn?: number | null;
  seq?: number | null;
  text?: string;
  by?: string | null;
  tool_kind?: string;
  title?: string;
  status?: string;
  summary?: string;
  content?: unknown[];
  request_id?: string;
  options?: { option_id: string; name: string; kind: string }[];
  outcome?: { option_id?: string; cancelled?: boolean; by?: string } | null;
  entries?: [string, string][];
  used?: number | null;
  size?: number | null;
  stop_reason?: string;
  unknown_kind?: string;
  payload?: unknown;
}

export interface ContextUsage {
  used: number;
  size: number;
  cost: { amount: number; currency: string } | null;
}

export type ChatRow<T extends ChatDisplayBlock = ChatDisplayBlock> =
  | {
      kind: "single";
      key: string;
      block: T;
      index: number;
    }
  | {
      /** The thought currently being thought: the live turn's trailing
       * thought, rendered open while the turn runs, folded once it ends. */
      kind: "live_thought";
      key: string;
      block: T;
      index: number;
    }
  | {
      /** Adjacent tool calls and finished thinking in the same turn. */
      kind: "work_group";
      key: string;
      blocks: T[];
      firstIndex: number;
      /** Stable identities for retaining disclosure state when a page joins a run. */
      memberKeys: string[];
      /** `thinking` while the turn is still in flight, else `done`. */
      state: "thinking" | "done";
      calls: T[];
      thoughts: T[];
      thinkingTokens: number;
    };

/** A journal-position key stays fixed when older blocks are prepended. */
export function blockKey(block: ChatDisplayBlock, index: number): string {
  if (block.turn != null && block.seq != null) {
    return `${block.turn}:${block.seq}`;
  }
  // Older backends omit coordinates. The fallback is only stable until the
  // loaded window changes; current fetch_chat includes both coordinates.
  return `index:${index}`;
}

/** The raw text a block's copy button puts on the clipboard. */
export function blockCopyText(block: ChatDisplayBlock): string {
  switch (block.kind) {
    case "thought":
      return block.text ?? "";
    case "user_message":
    case "agent_message":
      return (block.text ?? "").trim();
    case "plan":
      return formatPlanEntries(block.entries ?? []);
    default:
      return "";
  }
}

/** A plan's steps, one per line, status-prefixed like the rendered block. */
export function formatPlanEntries(entries: [string, string][]): string {
  return entries.map(([content, status]) => `[${status}] ${content}`).join("\n");
}

interface ToolCallPart {
  type?: unknown;
  text?: unknown;
  path?: unknown;
}

/** A single tool call's summary — its text runs, then diff paths. */
export function toolCallCopyText(call: ChatDisplayBlock): string {
  const parts: string[] = [];
  const title = call.title || call.tool_kind;
  if (title) parts.push(title);
  for (const part of (call.content ?? []) as ToolCallPart[]) {
    if (!part || typeof part !== "object") continue;
    if (part.type === "text" && typeof part.text === "string" && part.text) {
      parts.push(part.text);
    } else if (part.type === "diff" && typeof part.path === "string") {
      parts.push(`diff ${part.path}`);
    }
  }
  return parts.join("\n");
}

/** Estimated thinking tokens for the thoughts in one collapsed work
 * group. The journal's `usage` blocks are a cumulative context-window
 * gauge (`used/size` for the whole trace), not per-turn thinking, so they
 * must not be read as a thinking total; estimate from the group's own
 * thought text instead (~4 chars per token). */
export function countThinkingTokens(thoughts: readonly { text?: string }[]): number {
  const chars = thoughts.reduce((sum, thought) => sum + (thought.text ?? "").length, 0);
  return Math.round(chars / 4);
}

/** The newest journal usage block is the authoritative context gauge for the
 * conversation. Loom's session summary can retain the adapter's initial
 * `0/size` report even after later turns have journaled real usage. Preserve
 * summary-only cost data, and fall back to the summary for older journals. */
export function resolveContextUsage(
  blocks: readonly ChatDisplayBlock[],
  summary?: ContextUsage | null,
): ContextUsage | null {
  for (let index = blocks.length - 1; index >= 0; index--) {
    const block = blocks[index];
    if (
      block.kind === "usage" &&
      typeof block.used === "number" && Number.isFinite(block.used) && block.used >= 0 &&
      typeof block.size === "number" && Number.isFinite(block.size) && block.size > 0
    ) {
      return { used: block.used, size: block.size, cost: summary?.cost ?? null };
    }
  }
  return summary ?? null;
}

/** `123`, `12.5K`, `3.2M` — a human-friendly token count. */
export function formatTokens(tokens: number): string {
  if (tokens < 1000) return String(tokens);
  const units: [number, string][] = [[1e6, "M"], [1e3, "K"]];
  for (const [scale, suffix] of units) {
    if (tokens >= scale) {
      const value = tokens / scale;
      return `${value >= 10 ? Math.round(value) : Math.round(value * 10) / 10}${suffix}`;
    }
  }
  return String(tokens);
}

/** The turns pi-acp's command interceptor answered locally: the turn's
 * opening user message names a slash command from the session's catalog
 * (only the leading word matters — `/compact keep tests` compacts), and the
 * turn holds nothing but that message, the reply, and its turn_end. The
 * adapter answers such prompts without the model — no thoughts, tool
 * calls, or usage reports — so the journaled agent_message text is
 * interceptor output (stats, usage receipts, confirmations), not agent
 * prose. File commands (`.pi/commands/*.md`) share the catalog but expand
 * into prompts the model answers, and model turns always journal work or
 * usage, which the shape filter excludes. The UI can style these replies
 * as system messages. Returns turn number → command name. */
export function commandReplyTurns(
  blocks: readonly ChatDisplayBlock[],
  commands: readonly string[],
): Map<number, string> {
  const names = new Set(commands);
  const turns = new Map<number, string>();
  if (!names.size) return turns;
  // The opening user message settles a turn: it is journaled before any
  // reply, so the first one seen decides. A turn whose opening message is
  // not a catalog command can still hold later /command text (steering) —
  // that is model-facing prose, not an interceptor turn.
  const settled = new Set<number>();
  const foreign = new Set<number>();
  for (const block of blocks) {
    if (block.turn == null) continue;
    if (block.kind !== "user_message" && block.kind !== "agent_message" && block.kind !== "turn_end") {
      foreign.add(block.turn);
    }
    if (block.kind !== "user_message" || settled.has(block.turn)) continue;
    settled.add(block.turn);
    const name = (block.text ?? "").match(/^\s*\/(\S+)/)?.[1] ?? "";
    if (names.has(name)) turns.set(block.turn, name);
  }
  for (const turn of [...turns.keys()]) {
    if (foreign.has(turn)) turns.delete(turn);
  }
  return turns;
}

/** The thought still being thought, if any: the trailing non-usage block of
 * the live turn. Once tool calls or messages follow, thinking is over. */
function liveThoughtIndex(blocks: readonly ChatDisplayBlock[], liveTurn: number | null): number | null {
  if (liveTurn == null) return null;
  let last = -1;
  for (let i = 0; i < blocks.length; i++) {
    if (blocks[i].kind !== "usage") last = i;
  }
  const block = blocks[last];
  if (block?.kind === "thought" && (block.turn == null || block.turn === liveTurn)) return last;
  return null;
}

/** Which corner a block's copy icon pins to: "top" while the host's
 * top edge sits at or below the scroll viewport's top edge (its top-right
 * corner is on screen), "bottom" once the top has scrolled out of view —
 * so a block taller than the pane keeps its icon at the trailing edge.
 * Blocks entirely off screen keep whatever corner follows; only the
 * visible edge matters. */
export function copyCornerFor(hostTop: number, viewportTop: number): "top" | "bottom" {
  return hostTop >= viewportTop ? "top" : "bottom";
}

/** Group visually adjacent tool calls and finished thoughts (same turn) into
 * one collapsed work row — except the current thinking block while its turn
 * is in flight, which stays its own open row. Invisible usage blocks may
 * appear between members; callers can still read them from the original
 * block list when showing token usage. */
export function groupDisplayBlocks<T extends ChatDisplayBlock>(
  blocks: readonly T[],
  liveTurn?: number | null,
  repairInterruptedPiProse = false,
): ChatRow<T>[] {
  const rows: ChatRow<T>[] = [];
  const liveThought = liveThoughtIndex(blocks, liveTurn ?? null);
  // Older Loom journals flushed agent prose whenever a thought delta arrived.
  // Pi can send a stray thought between text deltas, leaving one sentence in
  // several agent_message blocks. Join prose within the same uninterrupted
  // assistant span; keep the thought blocks before the joined message.
  const hiddenMessages = new Set<number>();
  const joinedMessages = new Map<number, T>();
  for (let start = 0; repairInterruptedPiProse && start < blocks.length;) {
    const turn = blocks[start].turn;
    if (turn == null || !["agent_message", "thought", "usage"].includes(blocks[start].kind)) {
      start++;
      continue;
    }
    let end = start;
    const messages: number[] = [];
    let hasThought = false;
    while (end < blocks.length && blocks[end].turn === turn &&
      ["agent_message", "thought", "usage"].includes(blocks[end].kind)) {
      if (blocks[end].kind === "agent_message") messages.push(end);
      if (blocks[end].kind === "thought") hasThought = true;
      end++;
    }
    if (hasThought && messages.length > 1) {
      const last = messages[messages.length - 1];
      joinedMessages.set(last, {
        ...blocks[last],
        text: messages.map((index) => blocks[index].text ?? "").join(""),
      });
      for (const index of messages.slice(0, -1)) hiddenMessages.add(index);
    }
    start = end;
  }
  for (let index = 0; index < blocks.length; index++) {
    if (hiddenMessages.has(index)) continue;
    const block = joinedMessages.get(index) ?? blocks[index];
    if (block.kind === "usage") continue;
    if (index === liveThought) {
      rows.push({ kind: "live_thought", key: `block:${blockKey(block, index)}`, block, index });
      continue;
    }
    if (block.kind !== "tool_call" && block.kind !== "thought") {
      rows.push({ kind: "single", key: `block:${blockKey(block, index)}`, block, index });
      continue;
    }

    const firstIndex = index;
    const members: T[] = [block];
    const memberKeys = [blockKey(block, index)];
    while (index + 1 < blocks.length) {
      let nextIndex = index + 1;
      while (nextIndex === liveThought || hiddenMessages.has(nextIndex) || blocks[nextIndex]?.kind === "usage") nextIndex++;
      const next = blocks[nextIndex];
      if (!next) break;
      if (next.kind !== "tool_call" && next.kind !== "thought") break;
      // Coordinates from different turns imply a boundary even if a backend
      // omitted the turn_end block. Missing coordinates retain adjacency.
      if (block.turn != null && next.turn != null && block.turn !== next.turn) break;
      index = nextIndex;
      members.push(next);
      memberKeys.push(blockKey(next, index));
    }

    // While the turn is in flight the group is still growing behind the
    // live-thought row; label it so the status pill reads correctly.
    const state = liveTurn != null && members.some((m) => m.turn == null || m.turn === liveTurn)
      ? "thinking"
      : "done";
    const thoughts = members.filter((m) => m.kind === "thought");
    rows.push({
      kind: "work_group",
      key: `tools:${memberKeys[0]}`,
      blocks: members,
      firstIndex,
      memberKeys,
      state,
      calls: members.filter((m) => m.kind === "tool_call"),
      thoughts,
      thinkingTokens: countThinkingTokens(thoughts),
    });
  }
  return rows;
}
