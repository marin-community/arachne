// Markdown-preserving clipboard support for rendered chat messages.
//
// ChatMarkdown renders messages from their markdown source; the DOM only
// holds rendered HTML, so a native copy would flatten structure (code
// fences, lists, links). The raw source rides along on the rendered
// element (data-markdown); this module maps a selection over the rendered
// body back to a markdown source range so the clipboard can carry both
// flavors: text/plain as markdown (GitHub keeps the source) and text/html
// as the rendered markup (rich-text targets keep formatting).
//
// Whole-body selections are exact. Partial selections map rendered
// characters back to source: marked block tokens partition the source,
// and each token's rendered text is measured (from the real DOM when it
// pairs, else from a fresh render of the tokens) so a cut lands in the
// right block — exactly inside code blocks, whose content is verbatim in
// the source. Fenced code never splits into an unbalanced fence: a cut
// crossing a fenced block snaps to the whole block, and a selection
// contained in one yields just the code text.

import { Marked, type Tokens } from "marked";

// Must match ChatMarkdown's parse options so lexer blocks pair with the
// rendered DOM.
const marked = new Marked({ gfm: true, breaks: false, async: false });

/** The DOM surface this module needs: a rendered element's children. */
export interface DomLike {
  nodeType: number;
  data?: string;
  textContent: string | null;
  childNodes?: ArrayLike<DomLike> | null;
}

/** One rendered top-level block: its source span and rendered text span. */
export interface RenderedBlock {
  mdStart: number;
  mdEnd: number;
  textStart: number;
  textEnd: number;
  /** Set for code blocks: exact map from rendered offset to source. */
  code?: CodeMap;
}

/** Line anchors mapping a code block's rendered text to its source. */
export interface CodeMap {
  /** Rendered-text offsets (relative to the block's text start) where each
   * source line begins; code content is verbatim, so anchors are exact. */
  anchors: { text: number; src: number }[];
  /** Rendered content length (without marked's trailing rendered newline). */
  contentLength: number;
}

const FENCE_OPEN = /^ {0,3}(?:`{3,}|~{3,})/;
const FENCE_CLOSE = /^ {0,3}(?:`{3,}|~{3,})\s*$/;

/**
 * Blocks with rendered-text spans for a markdown source. Tries the real
 * DOM first (element-per-token pairing, the authoritative shape), then
 * falls back to rendering the tokens (handles DOM/sanitizer quirks like
 * raw HTML expanding to several elements), and only returns null when
 * neither lines up with the given body.
 */
export function buildRenderedBlocks(md: string, body: DomLike): RenderedBlock[] | null {
  const blocks = tokenBlocks(md);
  if (!blocks) return null;
  if (domPairs(body, blocks)) return blocks;
  // The DOM's total text still matches the token render (only the
  // element boundaries diverged); token spans locate offsets exactly.
  const total = (body.textContent ?? "").length;
  if (total === renderedTextLength(blocks)) return blocks;
  return null;
}

/** Sum of the blocks' rendered text lengths. */
function renderedTextLength(blocks: RenderedBlock[]): number {
  return blocks.reduce((sum, b) => sum + (b.textEnd - b.textStart), 0);
}

/** Does the DOM's element-per-token shape line up with the token blocks?
 * Each rendered block element carries the block's text without marked's
 * trailing newline; the newline itself lands between elements as a text
 * node (marked joins block HTML with "\n"), which the walk counts. */
function domPairs(body: DomLike, blocks: RenderedBlock[]): boolean {
  const children = Array.from(body.childNodes ?? []);
  let childIndex = 0;
  let textCursor = 0;
  const nextElement = (): DomLike | null => {
    while (childIndex < children.length) {
      const child = children[childIndex++];
      if (child.nodeType === 1) return child;
      if (child.nodeType === 3) textCursor += (child.data ?? "").length;
    }
    return null;
  };
  for (const block of blocks) {
    const el = nextElement();
    if (!el) return false;
    const text = el.textContent ?? "";
    const want = block.textEnd - block.textStart;
    if (textCursor !== block.textStart) return false;
    // The block's trailing rendered newline sits after the element as a
    // text node; consume it so the next block starts at its textStart.
    const inElement = want > 0 && text.length === want - 1 ? want - 1 : want;
    if (text.length !== inElement) return false;
    textCursor += text.length;
    while (childIndex < children.length && children[childIndex].nodeType !== 1) {
      if (children[childIndex].nodeType === 3) textCursor += (children[childIndex].data ?? "").length;
      childIndex++;
    }
    if (textCursor !== block.textEnd) return false;
  }
  // Remaining nodes must only be separator text.
  nextElement();
  return textCursor === (body.textContent ?? "").length;
}

/** Lex a source into blocks with rendered-text spans. */
function tokenBlocks(md: string): RenderedBlock[] | null {
  let tokens: Tokens.Generic[];
  try {
    tokens = marked.lexer(md) as Tokens.Generic[];
  } catch {
    return null;
  }
  const blocks: RenderedBlock[] = [];
  let mdCursor = 0;
  let textCursor = 0;
  for (const token of tokens) {
    const raw = token.raw ?? "";
    if (token.type === "space" || token.type === "def") {
      mdCursor += raw.length;
      continue;
    }
    let text: string;
    try {
      text = renderedText(token, raw);
    } catch {
      return null;
    }
    const block: RenderedBlock = {
      mdStart: mdCursor,
      mdEnd: mdCursor + raw.length,
      textStart: textCursor,
      textEnd: textCursor + text.length,
    };
    if (token.type === "code") block.code = buildCodeMap(block, raw);
    blocks.push(block);
    textCursor += text.length;
    mdCursor += raw.length;
  }
  return blocks;
}

/** The text a token renders to (tags stripped, entities decoded). The
 * parser output's trailing separator newline is included, so block text
 * spans tile the body's textContent exactly (separator newlines land
 * between elements in the DOM). */
function renderedText(token: Tokens.Generic, raw: string): string {
  let html: string;
  try {
    html = marked.parser([token] as Tokens.Generic[]) as unknown as string;
  } catch {
    return raw;
  }
  return decodeEntities(stripTags(html));
}

const ENTITIES: Record<string, string> = {
  amp: "&", lt: "<", gt: ">", quot: '"', apos: "'", nbsp: "\u00a0",
  copy: "\u00a9", reg: "\u00ae", trade: "\u2122", hellip: "\u2026",
  mdash: "\u2014", ndash: "\u2013", lsquo: "\u2018", rsquo: "\u2019",
  ldquo: "\u201c", rdquo: "\u201d", bull: "\u2022", middot: "\u00b7",
};

/** Decode the entities marked emits (numeric and the common named set). */
export function decodeEntities(html: string): string {
  return html.replace(/&(#[xX]?[0-9a-fA-F]+|[a-zA-Z]+);/g, (match, body: string) => {
    if (body.startsWith("#")) {
      const code = body[1] === "x" || body[1] === "X"
        ? parseInt(body.slice(2), 16)
        : parseInt(body.slice(1), 10);
      return Number.isFinite(code) && code > 0 && code <= 0x10ffff ? String.fromCodePoint(code) : match;
    }
    return ENTITIES[body] ?? match;
  });
}

function stripTags(html: string): string {
  return html.replace(/<[^>]*>/g, "");
}

/**
 * Anchor map for a code block. Fenced code content appears verbatim in
 * the source between the fence lines; indented code loses its leading
 * spaces/tab per line when rendered, so anchors land after each line's
 * indent.
 */
function buildCodeMap(block: RenderedBlock, raw: string): CodeMap {
  const r = raw.endsWith("\n") ? raw.slice(0, -1) : raw;
  const lines = r.split("\n");
  const anchors: { text: number; src: number }[] = [];
  if (FENCE_OPEN.test(r)) {
    const lastBreak = r.lastIndexOf("\n");
    const closeLine = lastBreak === -1 ? "" : r.slice(lastBreak + 1);
    // Unterminated fences treat everything after the opening line as
    // content so partial cuts never emit a dangling close fence.
    const contentEnd = lastBreak === -1 || !FENCE_CLOSE.test(closeLine)
      ? block.mdEnd
      : block.mdStart + lastBreak;
    let textOffset = 0;
    for (let i = 1; i < lines.length; i++) {
      const src = block.mdStart + (lines.slice(0, i).join("\n").length + 1);
      if (src > contentEnd) break;
      anchors.push({ text: textOffset, src });
      textOffset += lines[i].length + 1;
    }
    return { anchors, contentLength: Math.max(0, textOffset - 1) };
  }
  // Indented code: each source line drops its leading indent when rendered.
  let textOffset = 0;
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    const indentMatch = /^(?: {1,4}|\t)/.exec(line);
    const indent = indentMatch ? indentMatch[0].length : 0;
    const src = block.mdStart + (i === 0 ? 0 : lines.slice(0, i).join("\n").length + 1) + indent;
    anchors.push({ text: textOffset, src });
    textOffset += line.length - indent + 1;
  }
  return { anchors, contentLength: Math.max(0, textOffset - 1) };
}

/** Map a rendered offset inside a code block to an exact source offset. */
function codeSourceOffset(block: RenderedBlock, offset: number): number {
  const code = block.code!;
  const local = Math.max(0, Math.min(code.contentLength, offset - block.textStart));
  let anchor = code.anchors[0];
  for (const next of code.anchors) {
    if (next.text <= local) anchor = next;
    else break;
  }
  return anchor.src + (local - anchor.text);
}

export interface MarkdownRange {
  from: number;
  to: number;
}

/**
 * Map a rendered-text selection [startOff, endOff) of the body onto a
 * markdown source range. Offsets are characters into the body's
 * textContent; 0 and textContent.length mean the body's edges.
 */
export function markdownRangeFor(
  blocks: RenderedBlock[],
  md: string,
  startOff: number,
  endOff: number,
): MarkdownRange {
  if (blocks.length === 0) return { from: 0, to: md.length };
  const a = cutPosition(blocks, md, startOff);
  const b = cutPosition(blocks, md, endOff);
  const left = a.pos <= b.pos ? a : b;
  const right = a.pos <= b.pos ? b : a;
  let from = left.pos;
  let to = right.pos;
  // Fenced code never splits into an unbalanced fence: a cut crossing a
  // fenced block snaps to the whole block, so both fences stay or go.
  if (left.insideCode && left.block !== right.block) from = left.block!.mdStart;
  if (right.insideCode && right.block !== left.block) to = right.block!.mdEnd;
  return { from, to };
}

interface CutPosition {
  pos: number;
  block: RenderedBlock | null;
  insideCode: boolean;
}

/** Map one rendered offset to a source position (exactly inside code). */
function cutPosition(blocks: RenderedBlock[], md: string, offset: number): CutPosition {
  for (const b of blocks) {
    if (offset < b.textStart) {
      // The cut sits in the gap before this block: a clean block boundary.
      return { pos: b.mdStart, block: null, insideCode: false };
    }
    if (offset >= b.textEnd) continue;
    // The block's rendered text ends with a separator newline that lives
    // between elements in the DOM; a cut there is still inside the block,
    // but a cut at the separator's start (or anywhere in it for the last
    // block) reads as "to the block's visible end" for proportional spans.
    if (b.code) return { pos: codeSourceOffset(b, offset), block: b, insideCode: true };
    const textLen = b.textEnd - b.textStart;
    const local = Math.max(0, Math.min(textLen, offset - b.textStart));
    if (local >= textLen - 1) {
      // Past the visible text (in or at the separator): end of the block.
      return { pos: b.mdEnd, block: b, insideCode: false };
    }
    return { pos: proportional(b, offset), block: b, insideCode: false };
  }
  return { pos: md.length, block: null, insideCode: false };
}

function proportional(block: RenderedBlock, offset: number): number {
  const textLen = block.textEnd - block.textStart;
  const span = block.mdEnd - block.mdStart;
  if (textLen <= 0 || span <= 0) return block.mdStart;
  const local = Math.max(0, Math.min(textLen, offset - block.textStart));
  return block.mdStart + Math.round((local / textLen) * span);
}

/**
 * Markdown for a selection within one rendered body. `startOff`/`endOff`
 * are textContent-relative offsets (0 = body start, textLength = body
 * end). Falls back to proportional mapping when the DOM and source
 * cannot be paired.
 */
export function markdownForSelection(md: string, body: DomLike, startOff: number, endOff: number): string {
  const text = body.textContent ?? "";
  const lo = Math.max(0, Math.min(text.length, Math.min(startOff, endOff)));
  const hi = Math.max(0, Math.min(text.length, Math.max(startOff, endOff)));
  if (lo <= 0 && hi >= text.length) return md;
  if (lo === hi) return "";
  const blocks = buildRenderedBlocks(md, body);
  const range = blocks
    ? markdownRangeFor(blocks, md, lo, hi)
    : proportionalMarkdownRange(md, text.length, lo, hi);
  return md.slice(range.from, range.to);
}

/**
 * Fallback when the DOM and source cannot be paired: map rendered
 * characters to source proportionally. Structure may drift (markdown
 * characters that render away skew the ratio) but the result is plain
 * text.
 */
export function proportionalMarkdownRange(md: string, textLength: number, startOff: number, endOff: number): MarkdownRange {
  if (textLength <= 0 || !md) return { from: 0, to: md.length };
  const scale = md.length / textLength;
  const start = Math.max(0, Math.min(textLength, startOff));
  const end = Math.max(start, Math.min(textLength, endOff));
  return { from: Math.round(start * scale), to: Math.round(end * scale) };
}
