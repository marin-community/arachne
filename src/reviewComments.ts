/**
 * Inline review comments on diff lines (GitHub-style).
 *
 * Pure, dependency-free logic so it stays unit-testable with node:test and
 * reusable by ChangeReview without pulling Vue into the module graph.
 */

export interface ReviewCommentLine {
  kind: "context" | "addition" | "deletion";
  old_line: number | null;
  new_line: number | null;
}

export interface ReviewComment {
  /** Stable identity within one review panel session. */
  id: string;
  /** File path display string the comment was attached to. */
  path: string;
  /** Resolved line number: new_line preferred, old_line for deletions. */
  line: number;
  /** Diff line kind at the attachment point. */
  lineKind: "context" | "addition" | "deletion";
  /** `path:line` anchor shown to the user and embedded in the prompt. */
  anchor: string;
  /** Single-line verbatim comment text. */
  text: string;
  /** Present when the comment covers a multi-line range. */
  range?: { first: number; last: number };
}

let commentSequence = 0;

/** Resolve the display line: prefer the new-side line, fall back to old. */
export function commentLineNumber(line: ReviewCommentLine): number | null {
  return line.new_line ?? line.old_line;
}

/** `path:line` anchor, or null when a hunk line has no numbers at all. */
export function commentAnchor(path: string, line: ReviewCommentLine): string | null {
  const number = commentLineNumber(line);
  return number == null ? null : `${path}:${number}`;
}

/**
 * `path:first-last` anchor for a multi-line range comment. Both endpoints must
 * resolve; a single-line range degenerates to the `path:line` form.
 */
export function rangeAnchorOf(path: string, first: ReviewCommentLine, last: ReviewCommentLine): string | null {
  const firstNumber = commentLineNumber(first);
  const lastNumber = commentLineNumber(last);
  if (firstNumber == null || lastNumber == null) return null;
  const low = Math.min(firstNumber, lastNumber), high = Math.max(firstNumber, lastNumber);
  return low === high ? `${path}:${low}` : `${path}:${low}-${high}`;
}

/** A comment requires trimmed text and a resolvable anchor; null otherwise. */
export function commentFromLine(path: string, line: ReviewCommentLine, text: string): ReviewComment | null {
  const trimmed = text.trim();
  if (!trimmed) return null;
  const number = commentLineNumber(line);
  if (number == null) return null;
  // Single-line verbatim: a newline in pasted text cannot smuggle a bullet.
  const single = trimmed.replaceAll("\r\n", "\n").replaceAll("\r", "\n").replaceAll("\n", " ");
  return {
    id: `comment-${commentAnchor(path, line)}-${++commentSequence}`,
    path,
    line: number,
    lineKind: line.kind,
    anchor: `${path}:${number}`,
    text: single,
  };
}

/**
 * A range comment requires trimmed text and two endpoints with resolvable
 * line numbers; null otherwise. The anchor covers the full span, and the
 * comment pins to its first line so it renders under the range's start.
 */
export function commentFromRange(path: string, first: ReviewCommentLine, last: ReviewCommentLine, text: string): ReviewComment | null {
  const anchor = rangeAnchorOf(path, first, last);
  if (anchor == null) return null;
  const trimmed = text.trim();
  if (!trimmed) return null;
  // Single-line verbatim: a newline in pasted text cannot smuggle a bullet.
  const single = trimmed.replaceAll("\r\n", "\n").replaceAll("\r", "\n").replaceAll("\n", " ");
  const firstNumber = commentLineNumber(first)!;
  const lastNumber = commentLineNumber(last)!;
  const low = Math.min(firstNumber, lastNumber), high = Math.max(firstNumber, lastNumber);
  return {
    id: `comment-${anchor}-${++commentSequence}`,
    path,
    line: low,
    lineKind: first.kind,
    anchor,
    text: single,
    ...(low === high ? {} : { range: { first: low, last: high } }),
  };
}

/** Group comments by path preserving first-seen path order, then line order. */
export function groupCommentsByPath(comments: ReviewComment[]): Map<string, ReviewComment[]> {
  const groups = new Map<string, ReviewComment[]>();
  for (const comment of comments) {
    const list = groups.get(comment.path);
    if (list) list.push(comment);
    else groups.set(comment.path, [comment]);
  }
  for (const list of groups.values()) list.sort((a, b) => a.line - b.line);
  return groups;
}

/**
 * Compact markdown block for the agent. Groups by path (#### header), one
 * bullet per comment, anchor in backticks, em-dash separator, text verbatim.
 */
export function buildCommentPrompt(comments: ReviewComment[]): string {
  const usable = comments.filter(comment => comment.text.trim());
  if (!usable.length) return "";
  const groups = [...groupCommentsByPath(usable)].map(([path, list]) =>
    [`#### ${path}`, ...list.map(comment => `- \`${comment.anchor}\` — ${comment.text}`)].join("\n"));
  return [
    "<details>",
    `<summary>${usable.length} inline comment${usable.length === 1 ? "" : "s"} on the current review</summary>`,
    "",
    groups.join("\n\n"),
    "</details>",
  ].join("\n");
}
