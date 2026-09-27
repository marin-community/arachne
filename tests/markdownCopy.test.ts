// Tests for markdown copy-source mapping. A tiny DOM stand-in parses
// marked's real HTML output into DomLike nodes, so the pairing logic runs
// against the same shapes the webview produces.
import assert from "node:assert/strict";
import test from "node:test";
import { Marked } from "marked";
import { buildRenderedBlocks, markdownForSelection, markdownRangeFor } from "../src/markdownCopy.ts";

interface SimpleNode {
  nodeType: number;
  data?: string;
  textContent: string | null;
  childNodes?: SimpleNode[];
}

const element = (children: SimpleNode[], text: string): SimpleNode => ({
  nodeType: 1,
  childNodes: children,
  textContent: text,
});
const textNode = (data: string): SimpleNode => ({ nodeType: 3, data, textContent: data });

// Parse flat-ish HTML into nodes: marked emits block HTML with nested
// inline tags; we need per-block element boundaries, their inner text,
// and the whitespace text nodes between elements (a real DOM's
// textContent includes those).
function parseMarkedHtml(html: string): SimpleNode[] {
  const nodes: SimpleNode[] = [];
  let cursor = 0;
  while (cursor < html.length) {
    const open = html.indexOf("<", cursor);
    if (open === -1) {
      nodes.push(textNode(html.slice(cursor)));
      break;
    }
    if (open > cursor) nodes.push(textNode(html.slice(cursor, open)));
    const tagEnd = html.indexOf(">", open);
    if (tagEnd === -1) break;
    const tag = html.slice(open + 1, tagEnd).split(/[\s>]/)[0];
    const close = html.indexOf(`</${tag}>`, tagEnd);
    if (close === -1) break;
    const inner = html.slice(tagEnd + 1, close);
    const text = stripTags(inner);
    nodes.push(element([textNode(text)], text));
    cursor = close + tag.length + 3;
  }
  return nodes;
}

const ENTITIES: Record<string, string> = {
  amp: "&", lt: "<", gt: ">", quot: '"', apos: "'", nbsp: "\u00a0",
};

function stripTags(html: string): string {
  // Decode the entities marked emits, like a real DOM would for textContent.
  return html
    .replace(/<[^>]*>/g, "")
    .replace(/&(#x?[0-9a-fA-F]+|[a-zA-Z]+);/g, (m, body: string) => {
      if (body.startsWith("#")) {
        const code = body[1] === "x" || body[1] === "X" ? parseInt(body.slice(2), 16) : parseInt(body.slice(1), 10);
        return Number.isFinite(code) && code > 0 ? String.fromCodePoint(code) : m;
      }
      return ENTITIES[body] ?? m;
    });
}

const markedInstance = new Marked({ gfm: true, breaks: false, async: false });

function renderBody(md: string): SimpleNode {
  const html = markedInstance.parse(md) as string;
  const children = parseMarkedHtml(html);
  const text = children.map((c) => c.data ?? c.textContent ?? "").join("");
  return { nodeType: 1, childNodes: children, textContent: text };
}

function offsetOf(text: string, needle: string): number {
  return text.indexOf(needle);
}

test("whole-body selection returns the full source", () => {
  const md = "Hello **world**\n\n- one\n- two\n\n```js\nconst a = 1;\n```";
  const body = renderBody(md);
  assert.equal(markdownForSelection(md, body, 0, body.textContent!.length), md);
});

test("partial selection inside a paragraph maps proportionally", () => {
  const md = "first paragraph\n\nsecond paragraph";
  const body = renderBody(md);
  const text = body.textContent!;
  const start = offsetOf(text, "second");
  const out = markdownForSelection(md, body, start, text.length);
  assert.equal(out, "second paragraph");
});

test("selection from before a fenced block into its middle keeps the fence whole", () => {
  const md = "intro text\n\n```rust\nfn main() {}\nlet x = 2;\n```\n\nafter";
  const body = renderBody(md);
  const text = body.textContent!;
  const start = offsetOf(text, "intro");
  const insideCode = offsetOf(text, "fn main");
  const out = markdownForSelection(md, body, start, insideCode);
  assert.ok(out.startsWith("intro text"));
  assert.ok(out.includes("```rust"), "keeps opening fence: " + out);
  assert.ok(out.endsWith("```"), "keeps closing fence: " + out);
});

test("selection fully inside one code block returns only the code text", () => {
  const md = "intro\n\n```rust\nfn main() {}\n```\n\nafter";
  const body = renderBody(md);
  const text = body.textContent!;
  const start = offsetOf(text, "fn main");
  const end = offsetOf(text, "main()") + "main()".length;
  const out = markdownForSelection(md, body, start, end);
  assert.equal(out, "fn main()");
});

test("selection across code block boundaries keeps fences balanced", () => {
  const md = "before\n\n```js\nline1\nline2\n```\n\nafter";
  const body = renderBody(md);
  const text = body.textContent!;
  const start = offsetOf(text, "before");
  const end = offsetOf(text, "after") + "after".length;
  const out = markdownForSelection(md, body, start, end);
  assert.equal(out, md);
});

test("selection starting mid-code and ending past the block keeps fences", () => {
  const md = "before\n\n```js\nline1\nline2\n```\n\nafter";
  const body = renderBody(md);
  const text = body.textContent!;
  const start = offsetOf(text, "line2");
  const end = offsetOf(text, "after") + "after".length;
  const out = markdownForSelection(md, body, start, end);
  assert.ok(out.startsWith("```js"), "starts with opening fence: " + out);
  assert.ok(out.endsWith("after"));
});

test("pairing returns null when the DOM diverges from the source", () => {
  const md = "alpha\n\nbeta";
  const divergent: SimpleNode = {
    nodeType: 1,
    childNodes: [element([textNode("alpha")], "alpha"), element([textNode("beta")], "beta"), element([textNode("gamma")], "gamma")],
    textContent: "alphabetagamma",
  };
  const blocks = buildRenderedBlocks(md, divergent);
  assert.equal(blocks, null);
  // The proportional fallback is lossy on a divergent DOM (extra rendered
  // text skews the ratio); it must still return plain text near the cut.
  const out = markdownForSelection(md, divergent, 0, 5);
  assert.ok(out.length > 0);
  assert.ok(out.startsWith("alph"));
});

test("DOM with merged elements maps via token totals when the text lines up", () => {
  const md = "alpha\n\nbeta";
  // One element holding the full rendered text (e.g. sanitizer merging):
  // element pairing fails but the rendered-text total matches, so the
  // token blocks still map offsets exactly.
  const merged: SimpleNode = {
    nodeType: 1,
    childNodes: [element([textNode("alpha\n\nbeta\n")], "alpha\n\nbeta\n")],
    textContent: "alpha\n\nbeta\n",
  };
  const out = markdownForSelection(md, merged, 0, 5);
  assert.equal(out, "alpha");
});

test("task list markers survive a whole-list copy", () => {
  const md = "- [ ] first\n- [x] done";
  const body = renderBody(md);
  const text = body.textContent!;
  const out = markdownForSelection(md, body, 0, text.length);
  assert.equal(out, md);
});

test("empty and collapsed selections return empty markdown", () => {
  const md = "hello";
  const body = renderBody(md);
  const text = body.textContent!;
  assert.equal(markdownForSelection(md, body, 2, 2), "");
  assert.equal(markdownForSelection(md, body, 0, 0), "");
  assert.equal(markdownForSelection(md, body, text.length, text.length), "");
});

test("cut at a block boundary snaps to the block start", () => {
  const md = "one\n\ntwo\n\nthree";
  const body = renderBody(md);
  const text = body.textContent!;
  const start = offsetOf(text, "two");
  const end = offsetOf(text, "three") + "three".length;
  const out = markdownForSelection(md, body, start, end);
  assert.equal(out, "two\n\nthree");
});

test("code-only body: selection inside the fence yields code text", () => {
  const md = "```python\nprint('hi')\nprint('bye')\n```";
  const body = renderBody(md);
  const text = body.textContent!;
  const start = offsetOf(text, "print('hi')");
  const end = offsetOf(text, "print('bye')") + "print('bye')".length;
  const out = markdownForSelection(md, body, start, end);
  assert.equal(out, "print('hi')\nprint('bye')");
});

test("markdownRangeFor handles an empty block list", () => {
  assert.deepEqual(markdownRangeFor([], "abc"), { from: 0, to: 3 });
});
