import assert from "node:assert/strict";
import test from "node:test";
import { effectScope, nextTick, ref } from "vue";
import { useFileCompletion, type FileCompletionSource } from "../src/useFileCompletion.ts";

const settle = async () => { await nextTick(); await new Promise((resolve) => setTimeout(resolve, 180)); };

function composer(lookup: (source: FileCompletionSource, query: string) => Promise<string[]>) {
  const scope = effectScope();
  const draft = ref("");
  const source = ref<FileCompletionSource | null>({ repo: "/repo" });
  const textarea = {
    selectionStart: 0,
    focus() {},
    setSelectionRange(start: number) { this.selectionStart = start; },
  };
  const input = ref(textarea);
  const completion = scope.run(() => useFileCompletion(draft, source, input as any, lookup))!;
  function type(text: string, caret = text.length) {
    draft.value = text;
    textarea.selectionStart = caret;
    completion.updateCaret();
  }
  return { scope, draft, source, textarea, completion, type };
}

function key(name: string, modifiers = {}) {
  let prevented = false;
  return {
    event: { key: name, preventDefault() { prevented = true; }, stopPropagation() {}, ...modifiers } as KeyboardEvent,
    prevented: () => prevented,
  };
}

test("new-track completion queries the selected repository and inserts a file at the actual caret", async (t) => {
  const requests: unknown[] = [];
  const c = composer(async (source, query) => { requests.push({ source, query }); return ["src/fresh.ts"]; });
  t.after(() => c.scope.stop());
  c.type("Inspect @fre and keep this", "Inspect @fre".length);
  await settle();
  assert.deepEqual(requests, [{ source: { repo: "/repo" }, query: "fre" }]);
  assert.equal(c.completion.visible.value, true);
  const tab = key("Tab");
  assert.equal(c.completion.onKeydown(tab.event), true);
  assert.equal(tab.prevented(), true);
  await nextTick();
  assert.equal(c.draft.value, "Inspect @src/fresh.ts and keep this");
  assert.equal(c.textarea.selectionStart, "Inspect @src/fresh.ts".length);
  assert.equal(c.completion.visible.value, false);
});

test("file completion leaves modified Enter to send/newline and reserves braced resource mentions", async (t) => {
  const c = composer(async () => ["README.md"]);
  t.after(() => c.scope.stop());
  c.source.value = { id: "thread" };
  c.type("@read");
  await settle();
  for (const modifiers of [{ ctrlKey: true }, { metaKey: true }, { shiftKey: true }]) {
    const enter = key("Enter", modifiers);
    assert.equal(c.completion.onKeydown(enter.event), false);
    assert.equal(enter.prevented(), false);
  }
  c.type("@{Design document");
  await settle();
  assert.equal(c.completion.visible.value, false);
});

test("changing repositories discards an in-flight response from the previous checkout", async (t) => {
  let finishOld!: (files: string[]) => void;
  const c = composer(async (source) => source.repo === "/repo"
    ? new Promise<string[]>((resolve) => { finishOld = resolve; }) : ["new/file.ts"]);
  t.after(() => c.scope.stop());
  c.type("@file");
  await settle();
  c.source.value = { repo: "/new" };
  await settle();
  finishOld(["old/file.ts"]);
  await nextTick();
  assert.deepEqual(c.completion.matches.value, ["new/file.ts"]);
});

test("a failed file lookup leaves the draft usable", async (t) => {
  const c = composer(async () => { throw new Error("offline"); });
  t.after(() => c.scope.stop());
  c.type("Please inspect @src/file.ts");
  await settle();
  assert.equal(c.completion.visible.value, false);
  assert.equal(c.draft.value, "Please inspect @src/file.ts");
});

test("Escape keeps a pending file lookup from reopening the menu", async (t) => {
  let finish!: (files: string[]) => void;
  const c = composer(() => new Promise<string[]>((resolve) => { finish = resolve; }));
  t.after(() => c.scope.stop());
  c.type("@file");
  await settle();
  const escape = key("Escape");
  assert.equal(c.completion.onKeydown(escape.event), true);
  finish(["file.ts"]);
  await nextTick();
  assert.equal(c.completion.visible.value, false);
  assert.equal(c.draft.value, "@file");
});
