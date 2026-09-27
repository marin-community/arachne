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
  entries?: [string, string][];
  used?: number | null;
  size?: number | null;
  stop_reason?: string;
  unknown_kind?: string;
  payload?: unknown;
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

/** Token totals for a turn: chars from its finished thoughts, input+output
 * tokens from its usage blocks. Codex journals thinking as context usage, so
 * the usage numbers already cover it — prefer them when present. */
export function countThinkingTokens(blocks: readonly ChatDisplayBlock[], turn: number | null | undefined): number {
  let chars = 0;
  let used = 0;
  let sawUsage = false;
  for (const block of blocks) {
    if (turn != null && block.turn != null && block.turn !== turn) continue;
    if (block.kind === "thought") chars += (block.text ?? "").length;
    else if (block.kind === "usage" && block.used != null && block.used > 0) {
      sawUsage = true;
      used += block.used;
    }
  }
  return sawUsage ? used : Math.round(chars / 4);
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
): ChatRow<T>[] {
  const rows: ChatRow<T>[] = [];
  const liveThought = liveThoughtIndex(blocks, liveTurn ?? null);
  for (let index = 0; index < blocks.length; index++) {
    const block = blocks[index];
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
      while (nextIndex === liveThought || blocks[nextIndex]?.kind === "usage") nextIndex++;
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
    rows.push({
      kind: "work_group",
      key: `tools:${memberKeys[0]}`,
      blocks: members,
      firstIndex,
      memberKeys,
      state,
      calls: members.filter((m) => m.kind === "tool_call"),
      thoughts: members.filter((m) => m.kind === "thought"),
      thinkingTokens: countThinkingTokens(blocks, block.turn),
    });
  }
  return rows;
}
