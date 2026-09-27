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
      kind: "tool_call_group";
      key: string;
      blocks: T[];
      firstIndex: number;
      /** Stable identities for retaining disclosure state when a page joins a run. */
      memberKeys: string[];
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

/** Group visually adjacent tool calls in the same turn. Invisible usage
 * blocks may appear between calls; callers can still read them from the
 * original block list when showing token usage. */
export function groupDisplayBlocks<T extends ChatDisplayBlock>(blocks: readonly T[]): ChatRow<T>[] {
  const rows: ChatRow<T>[] = [];
  for (let index = 0; index < blocks.length; index++) {
    const block = blocks[index];
    if (block.kind === "usage") continue;
    if (block.kind !== "tool_call") {
      rows.push({ kind: "single", key: `block:${blockKey(block, index)}`, block, index });
      continue;
    }

    const firstIndex = index;
    const calls: T[] = [block];
    const memberKeys = [blockKey(block, index)];
    while (index + 1 < blocks.length) {
      let nextIndex = index + 1;
      while (blocks[nextIndex]?.kind === "usage") nextIndex++;
      const next = blocks[nextIndex];
      if (!next) break;
      if (next.kind !== "tool_call") break;
      // Coordinates from different turns imply a boundary even if a backend
      // omitted the turn_end block. Missing coordinates retain adjacency.
      if (block.turn != null && next.turn != null && block.turn !== next.turn) break;
      index = nextIndex;
      calls.push(next);
      memberKeys.push(blockKey(next, index));
    }
    rows.push({
      kind: "tool_call_group",
      key: `tools:${memberKeys[0]}`,
      blocks: calls,
      firstIndex,
      memberKeys,
    });
  }
  return rows;
}
