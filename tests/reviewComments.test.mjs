import assert from "node:assert/strict";
import test from "node:test";
import { buildCommentPrompt, commentAnchor, commentFromLine, commentLineNumber, groupCommentsByPath } from "../src/reviewComments.ts";

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
