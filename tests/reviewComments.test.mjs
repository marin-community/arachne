import assert from "node:assert/strict";
import test from "node:test";
import { buildCommentPrompt, commentAnchor, commentFromLine, commentFromRange, commentLineNumber, groupCommentsByPath, rangeAnchorOf } from "../src/reviewComments.ts";

const context = (old_line, new_line) => ({ kind: "context", old_line, new_line });
const addition = (old_line, new_line) => ({ kind: "addition", old_line, new_line });
const deletion = (old_line, new_line) => ({ kind: "deletion", old_line, new_line });

test("comment line numbers prefer the new side and fall back to old for deletions", () => {
  assert.equal(commentLineNumber(addition(4, 5)), 5);
  assert.equal(commentLineNumber(context(4, 5)), 5);
  assert.equal(commentLineNumber(deletion(7, null)), 7);
  assert.equal(commentLineNumber({ kind: "context", old_line: null, new_line: null }), null);
});
test("anchors combine the path with the resolved line number", () => {
  assert.equal(commentAnchor("src/app.ts", addition(4, 5)), "src/app.ts:5");
  assert.equal(commentAnchor("src/app.ts", deletion(7, null)), "src/app.ts:7");
  assert.equal(commentAnchor("src/app.ts", { kind: "context", old_line: null, new_line: null }), null);
});
test("commentFromLine rejects blank and whitespace-only text and numberless lines", () => {
  assert.equal(commentFromLine("src/app.ts", addition(4, 5), ""), null);
  assert.equal(commentFromLine("src/app.ts", addition(4, 5), "   \n\t "), null);
  assert.equal(commentFromLine("src/app.ts", { kind: "context", old_line: null, new_line: null }, "Wait"), null);
  assert.equal(commentFromLine("src/app.ts", addition(4, 5), "   padded   ").text, "padded");
});
test("commentFromLine collapses pasted newlines to keep each bullet single-line", () => {
  assert.equal(commentFromLine("src/app.ts", addition(1, 2), "first\r\nsecond\nthird").text, "first second third");
  assert.equal(commentFromLine("src/app.ts", addition(1, 2), "a\rb").text, "a b");
});
test("comment ids stay unique for the same anchor across repeated additions", () => {
  const one = commentFromLine("src/app.ts", addition(1, 2), "First");
  const two = commentFromLine("src/app.ts", addition(1, 2), "Second");
  assert.notEqual(one.id, two.id);
  assert.equal(one.anchor, two.anchor);
});
test("groupCommentsByPath preserves first-seen path order and sorts lines within each file", () => {
  const first = commentFromLine("a.ts", addition(1, 10), "First");
  const second = commentFromLine("b.ts", deletion(3, null), "Second");
  const third = commentFromLine("a.ts", addition(2, 4), "Third");
  const groups = groupCommentsByPath([first, second, third]);
  assert.deepEqual([...groups.keys()], ["a.ts", "b.ts"]);
  assert.deepEqual(groups.get("a.ts").map(comment => comment.line), [4, 10]);
  assert.deepEqual(groups.get("b.ts").map(comment => comment.line), [3]);
});
test("buildCommentPrompt renders one path header per file with anchors and verbatim texts", () => {
  const comments = [
    commentFromLine("src/components/App.vue", addition(4, 12), "Something looks off here."),
    commentFromLine("src/lib.rs", deletion(3, null), "Do we need this flag at all?"),
    commentFromLine("src/components/App.vue", addition(20, 30), "Needs a test."),
  ];
  const prompt = buildCommentPrompt(comments);
  assert.ok(prompt.startsWith("<details>\n<summary>3 inline comments on the current review</summary>"));
  assert.ok(prompt.endsWith("</details>"));
  assert.ok(prompt.includes("#### src/components/App.vue"));
  assert.ok(prompt.includes("#### src/lib.rs"));
  assert.ok(prompt.includes("- `src/components/App.vue:12` — Something looks off here."));
  assert.ok(prompt.includes("- `src/components/App.vue:30` — Needs a test."));
  assert.ok(prompt.includes("- `src/lib.rs:3` — Do we need this flag at all?"));
  const appHeader = prompt.indexOf("#### src/components/App.vue");
  const libHeader = prompt.indexOf("#### src/lib.rs");
  assert.ok(appHeader < libHeader, "first-seen path order is preserved");
  assert.ok(prompt.indexOf("src/components/App.vue:12") < prompt.indexOf("src/components/App.vue:30"), "lines sort within a file");
});
test("buildCommentPrompt stays singular for one comment and empty without usable comments", () => {
  assert.equal(buildCommentPrompt([]), "");
  assert.equal(buildCommentPrompt([{ id: "x", path: "a.ts", line: 1, lineKind: "addition", anchor: "a.ts:1", text: "   " }]), "");
  const prompt = buildCommentPrompt([commentFromLine("a.ts", addition(1, 1), "Only")]);
  assert.ok(prompt.includes("1 inline comment on the current review"));
  assert.ok(prompt.includes("- `a.ts:1` — Only"));
});
test("rangeAnchorOf spans endpoints, orders them and degenerates to a single line", () => {
  assert.equal(rangeAnchorOf("src/foo.ts", context(10, 12), context(25, 27)), "src/foo.ts:12-27");
  // Selecting bottom-up yields the same ordered range.
  assert.equal(rangeAnchorOf("src/foo.ts", context(25, 27), context(10, 12)), "src/foo.ts:12-27");
  assert.equal(rangeAnchorOf("src/foo.ts", context(12, 12), addition(null, 27)), "src/foo.ts:12-27");
  // Deletion-only ranges fall back to old-side numbers.
  assert.equal(rangeAnchorOf("src/foo.ts", deletion(2, null), deletion(9, null)), "src/foo.ts:2-9");
  // A single-line selection degenerates to the single-line anchor form.
  assert.equal(rangeAnchorOf("src/foo.ts", addition(4, 12), addition(4, 12)), "src/foo.ts:12");
});
test("rangeAnchorOf rejects endpoints without resolvable line numbers", () => {
  const numberless = { kind: "context", old_line: null, new_line: null };
  assert.equal(rangeAnchorOf("src/foo.ts", numberless, addition(null, 3)), null);
  assert.equal(rangeAnchorOf("src/foo.ts", addition(null, 3), numberless), null);
});
test("commentFromRange builds an anchored range comment pinned to its first line", () => {
  const comment = commentFromRange("src/foo.ts", context(10, 12), context(25, 27), "Extract this block.");
  assert.equal(comment.anchor, "src/foo.ts:12-27");
  assert.deepEqual(comment.range, { first: 12, last: 27 });
  assert.equal(comment.path, "src/foo.ts");
  assert.equal(comment.line, 12);
  assert.equal(comment.lineKind, "context");
  assert.equal(comment.text, "Extract this block.");
  // Bottom-up selections anchor on the same ordered range.
  assert.deepEqual(commentFromRange("src/foo.ts", context(25, 27), context(10, 12), "Up").range, { first: 12, last: 27 });
  // Deletion-heavy ranges fall back to old-side numbers end to end.
  assert.deepEqual(commentFromRange("src/foo.ts", deletion(2, null), deletion(9, null), "Why").range, { first: 2, last: 9 });
});
test("commentFromRange rejects blank text and numberless endpoints and collapses newlines", () => {
  const numberless = { kind: "context", old_line: null, new_line: null };
  assert.equal(commentFromRange("src/foo.ts", context(10, 12), context(25, 27), ""), null);
  assert.equal(commentFromRange("src/foo.ts", context(10, 12), context(25, 27), "  \n "), null);
  assert.equal(commentFromRange("src/foo.ts", numberless, addition(null, 3), "Wait"), null);
  assert.equal(commentFromRange("src/foo.ts", addition(null, 3), numberless, "Wait"), null);
  assert.equal(commentFromRange("src/foo.ts", context(10, 12), context(25, 27), "a\r\nb").text, "a b");
});
test("commentFromRange degenerates to a single line without a range marker", () => {
  const single = commentFromRange("src/foo.ts", addition(4, 12), addition(4, 12), "Just this line");
  assert.equal(single.anchor, "src/foo.ts:12");
  assert.equal(single.range, undefined);
  assert.equal(single.text, "Just this line");
});
test("range comment ids stay unique for the same anchor across repeated additions", () => {
  const one = commentFromRange("src/app.ts", context(1, 2), context(5, 6), "First");
  const two = commentFromRange("src/app.ts", context(1, 2), context(5, 6), "Second");
  assert.notEqual(one.id, two.id);
  assert.equal(one.anchor, two.anchor);
});
test("range comments sort within their file and render their anchors in the prompt", () => {
  const range = commentFromRange("src/foo.ts", context(10, 12), context(25, 27), "Extract this block.");
  const single = commentFromLine("src/foo.ts", addition(4, 5), "Small fix.");
  assert.notEqual(range.id, single.id);
  const groups = groupCommentsByPath([range, single]);
  assert.deepEqual(groups.get("src/foo.ts").map(comment => comment.line), [5, 12]);
  const prompt = buildCommentPrompt([range, single]);
  assert.ok(prompt.includes("- `src/foo.ts:12-27` — Extract this block."));
  assert.ok(prompt.includes("- `src/foo.ts:5` — Small fix."));
});
