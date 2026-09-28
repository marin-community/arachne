// Tests for the selection → offset DOM glue that feeds markdown copy.
// Node has no DOM, so a minimal spec-faithful Range stand-in models the
// pieces bodyOffset uses: boundary points are (container, offset) pairs —
// child index for elements, character index for text nodes — and
// toString() yields the text between boundaries in document order.
// The regression this guards: the end edge once returned the suffix
// length (selection end → body end) instead of the selection end's own
// offset, so partial selections copied the wrong text.
import assert from "node:assert/strict";
import test from "node:test";
import { bodyOffset } from "../src/selectionOffsets.ts";
import { markdownForSelection } from "../src/markdownCopy.ts";

class TestNode {
  nodeType: number;
  data: string;
  childNodes: TestNode[];
  parent: TestNode | null;
  textContent: string;
  constructor(nodeType: number, data = "", children: TestNode[] = []) {
    this.nodeType = nodeType;
    this.data = data;
    this.childNodes = children;
    for (const child of children) child.parent = this;
    this.textContent = nodeType === 3 ? data : children.map((c) => c.textContent).join("");
  }
}

const el = (children: TestNode[]) => new TestNode(1, "", children);
const tx = (data: string) => new TestNode(3, data);

// Total text inside the root strictly before this node begins.
function beforeText(node: TestNode): number {
  if (!node.parent) return 0;
  const siblings = node.parent.childNodes;
  let total = 0;
  for (let i = 0; i < siblings.indexOf(node); i++) total += siblings[i].textContent.length;
  return beforeText(node.parent) + total;
}

// A boundary point's position in the root's concatenated text.
function boundaryPos(container: TestNode, offset: number): number {
  if (container.nodeType === 3) return beforeText(container) + Math.max(0, Math.min(container.data.length, offset));
  let pos = beforeText(container);
  for (let i = 0; i < offset && i < container.childNodes.length; i++) pos += container.childNodes[i].textContent.length;
  return pos;
}

function rootOf(node: TestNode): TestNode {
  let cur = node;
  while (cur.parent) cur = cur.parent;
  return cur;
}

const START_TO_START = 0;
const START_TO_END = 1;
const END_TO_END = 2;
const END_TO_START = 3;

class StandinRange {
  startContainer: TestNode;
  startOffset: number;
  endContainer: TestNode;
  endOffset: number;
  constructor(root: TestNode) {
    this.startContainer = root;
    this.startOffset = 0;
    this.endContainer = root;
    this.endOffset = 0;
  }
  setStart(node: TestNode, offset: number) {
    this.startContainer = node;
    this.startOffset = offset;
  }
  setEnd(node: TestNode, offset: number) {
    this.endContainer = node;
    this.endOffset = offset;
  }
  selectNodeContents(node: TestNode) {
    this.setStart(node, 0);
    this.setEnd(node, node.childNodes.length);
  }
  compareBoundaryPoints(how: number, source: StandinRange): number {
    const thisPos = how === START_TO_START || how === START_TO_END
      ? boundaryPos(this.startContainer, this.startOffset)
      : boundaryPos(this.endContainer, this.endOffset);
    const sourcePos = how === START_TO_START || how === END_TO_START
      ? boundaryPos(source.startContainer, source.startOffset)
      : boundaryPos(source.endContainer, source.endOffset);
    return thisPos < sourcePos ? -1 : thisPos > sourcePos ? 1 : 0;
  }
  toString(): string {
    const total = rootOf(this.startContainer).textContent;
    const from = boundaryPos(this.startContainer, this.startOffset);
    const to = boundaryPos(this.endContainer, this.endOffset);
    return from >= to ? "" : total.slice(from, to);
  }
}

// bodyOffset uses the DOM globals; the stand-in supplies them.
const root = el([]);
(globalThis as { document?: unknown }).document = { createRange: () => new StandinRange(root) };
(globalThis as { Range?: unknown }).Range = {
  START_TO_START, START_TO_END, END_TO_END, END_TO_START,
};

// A selection from (node, offset) to (node2, offset2).
function selection(from: [TestNode, number], to: [TestNode, number]): StandinRange {
  const range = new StandinRange(rootOf(from[0]));
  range.setStart(from[0], from[1]);
  range.setEnd(to[0], to[1]);
  return range;
}

test("partial selection inside one paragraph returns its true offsets", () => {
  // "Hello world foo", selecting "world" = [6, 11). The end edge used to
  // return the suffix length (15 - 11 = 4) instead of 11.
  const text = tx("Hello world foo");
  const body = el([el([text])]);
  const range = selection([text, 6], [text, 11]);
  assert.equal(bodyOffset(range, body, "start"), 6);
  assert.equal(bodyOffset(range, body, "end"), 11);
});

test("end edge maps across block elements and nested inlines", () => {
  // <p><strong>Hello</strong> world</p>\n<p>second para</p>
  // textContent: "Hello world\nsecond para" (24). Selection from "lo" in
  // the strong to "second| para" in the last block: [3, 18).
  const strongText = tx("Hello");
  const firstP = el([el([strongText]), tx(" world")]);
  const lastText = tx("second para");
  const body = el([firstP, tx("\n"), el([lastText])]);
  const range = selection([strongText, 3], [lastText, 6]);
  assert.equal(bodyOffset(range, body, "start"), 3);
  assert.equal(bodyOffset(range, body, "end"), 18);
});

test("edges outside the body clamp to 0 and full length", () => {
  // Selection starting before the body and ending inside it, and the
  // reverse: start edge clamps to 0, end edge to the body's length.
  const earlier = tx("earlier");
  const bodyText = tx("Hello world foo");
  const body = el([el([bodyText])]);
  const later = tx("later");
  const conv = el([el([earlier]), body, el([later])]);

  const into = selection([earlier, 2], [bodyText, 4]);
  assert.equal(bodyOffset(into, body, "start"), 0);
  assert.equal(bodyOffset(into, body, "end"), 4);

  const outOf = selection([bodyText, 2], [later, 3]);
  assert.equal(bodyOffset(outOf, body, "start"), 2);
  assert.equal(bodyOffset(outOf, body, "end"), 15);

  // A selection spanning the whole body clamps both edges.
  const whole = selection([earlier, 0], [later, 5]);
  assert.equal(bodyOffset(whole, body, "start"), 0);
  assert.equal(bodyOffset(whole, body, "end"), 15);
});

test("element-container boundaries resolve by child index", () => {
  // End boundary at (body, 1): right after the first <p> — the classic
  // triple-click shape. textContent: "Hello\nworld".
  const firstText = tx("Hello");
  const secondText = tx("world");
  const body = el([el([firstText]), tx("\n"), el([secondText])]);
  const range = selection([firstText, 2], [body, 1]);
  assert.equal(bodyOffset(range, body, "start"), 2);
  assert.equal(bodyOffset(range, body, "end"), 5);
});

test("selection spanning two bodies maps to each body's overlap", () => {
  // What onConversationCopy computes per body when a selection crosses
  // message boundaries: the first body clips its end, the second its start.
  const aText = tx("alpha beta");
  const a = el([el([aText])]);
  const bText = tx("gamma delta");
  const b = el([el([bText])]);
  const conv = el([a, tx("\n"), b]);
  const range = selection([aText, 6], [bText, 5]);
  assert.equal(bodyOffset(range, a, "start"), 6);
  assert.equal(bodyOffset(range, a, "end"), 10);
  assert.equal(bodyOffset(range, b, "start"), 0);
  assert.equal(bodyOffset(range, b, "end"), 5);
  assert.equal(conv.textContent.length, 22); // sanity: offsets share this space
});

test("copy composition: a partial selection copies the selected markdown", () => {
  // The user-visible bug: selecting "world foo … second" copied the wrong
  // text because the end offset was the suffix length. Run the full path
  // the copy handler uses — bodyOffset feeding markdownForSelection —
  // against a marked-shaped DOM.
  const md = "Hello world foo\n\nsecond paragraph";
  const firstText = tx("Hello world foo");
  const secondText = tx("second paragraph");
  const body = el([el([firstText]), tx("\n"), el([secondText])]);
  // Select from "world" through "second".
  const range = selection([firstText, 6], [secondText, 6]);
  const out = markdownForSelection(
    md,
    body,
    bodyOffset(range, body, "start"),
    bodyOffset(range, body, "end"),
  );
  assert.equal(out, "world foo\n\nsecond");
});
