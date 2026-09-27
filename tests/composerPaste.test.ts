// Tests for the composers' paste-flavor decision: a clipboard that carries
// text/plain (markdown source) pastes as plain text so the composer never
// receives WebKit's flattened rendering of the text/html flavor.
import assert from "node:assert/strict";
import test from "node:test";
import { pasteAsPlainText } from "../src/composerPaste.ts";

interface ItemLike {
  kind: string;
}

/** A DataTransfer stand-in: the paste handler only reads types, files,
 * items, and getData — exactly what a ClipboardEvent's clipboardData
 * exposes synchronously. */
function clipboard({
  types = [],
  text = null,
  files = 0,
  fileItems = 0,
}: {
  types?: string[];
  text?: string | null;
  files?: number;
  fileItems?: number;
}): ClipboardEvent {
  const data = {
    types,
    files: { length: files },
    items: Array.from({ length: fileItems }, () => ({ kind: "file" })) as ItemLike[],
    getData: (type: string) => (type === "text/plain" && text !== null ? text : ""),
  };
  return { clipboardData: data } as unknown as ClipboardEvent;
}

test("markdown flavors paste as plain text", () => {
  // A copied turn: text/plain = markdown, text/html = rendered markup.
  const event = clipboard({
    types: ["text/plain", "text/html"],
    text: "- bullet\n\n```rust\nfn main() {}\n```",
  });
  assert.equal(pasteAsPlainText(event), true);
});

test("text-only clipboard pastes as plain text", () => {
  const event = clipboard({ types: ["text/plain"], text: "plain words" });
  assert.equal(pasteAsPlainText(event), true);
});

test("empty text/plain falls through (no content to paste)", () => {
  const event = clipboard({ types: ["text/plain", "text/html"], text: "" });
  assert.equal(pasteAsPlainText(event), false);
});

test("image paste falls through to the file path", () => {
  const event = clipboard({ types: ["Files"], files: 1 });
  assert.equal(pasteAsPlainText(event), false);
});

test("clipboard-item image (no DataTransfer.files) falls through", () => {
  const event = clipboard({ types: ["Files"], fileItems: 1 });
  assert.equal(pasteAsPlainText(event), false);
});

test("html-only clipboard falls through to the default paste", () => {
  // No text/plain at all: pasting from an app that only writes HTML keeps
  // the webview's default behavior.
  const event = clipboard({ types: ["text/html"] });
  assert.equal(pasteAsPlainText(event), false);
});

test("no clipboardData is not a plain-text paste", () => {
  const event = { clipboardData: null } as unknown as ClipboardEvent;
  assert.equal(pasteAsPlainText(event), false);
});
