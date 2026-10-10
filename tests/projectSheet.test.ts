import assert from "node:assert/strict";
import test from "node:test";
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { compileScript, parse } from "@vue/compiler-sfc";
import { createRenderer } from "vue";
import ts from "typescript";

// Mount the real sheet's setup with Vue lifecycle hooks. Its render is a
// placeholder because these checks exercise keyboard ownership, not layout.
const require = createRequire(import.meta.url);
const source = readFileSync(new URL("../src/components/ProjectSheet.vue", import.meta.url), "utf8");
const script = compileScript(parse(source).descriptor, { id: "project-sheet-test" }).content;
const compiled = ts.transpileModule(script, {
  compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2021, esModuleInterop: true },
}).outputText;

function mountSheet() {
  const previousStorage = Object.getOwnPropertyDescriptor(globalThis, "localStorage");
  const previousDocument = Object.getOwnPropertyDescriptor(globalThis, "document");
  const document = new EventTarget();
  const values = new Map<string, string>();
  Object.defineProperty(globalThis, "document", { configurable: true, value: document });
  Object.defineProperty(globalThis, "localStorage", { configurable: true, value: {
    getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => values.set(key, value),
  } });
  const module = { exports: {} as any };
  new Function("require", "module", "exports", compiled)((name: string) => {
    if (name === "@tauri-apps/api/core") return { invoke: async () => [] };
    if (name.endsWith(".vue")) return {};
    if (name.startsWith("../")) return require(new URL(`${name.replace("../", "../src/")}.ts`, import.meta.url).pathname);
    return require(name);
  }, module, module.exports);
  const renderer = createRenderer<any, any>({
    createElement: () => ({}), createText: () => ({}), createComment: () => ({}),
    insert() {}, remove() {}, setText() {}, setElementText() {}, patchProp() {},
    parentNode: () => null, nextSibling: () => null,
  });
  let closes = 0;
  const app = renderer.createApp({ ...module.exports.default, render: () => null }, { onClose: () => closes++ });
  const instance = app.mount({}) as any;
  return {
    document, state: instance.$.setupState, closes: () => closes,
    stop() {
      app.unmount();
      for (const [key, descriptor] of [["localStorage", previousStorage], ["document", previousDocument]] as const) {
        if (descriptor) Object.defineProperty(globalThis, key, descriptor);
        else Reflect.deleteProperty(globalThis, key);
      }
    },
  };
}

function escape() {
  const event = new Event("keydown", { cancelable: true, bubbles: true });
  Object.defineProperty(event, "key", { value: "Escape" });
  return event;
}

test("Escape in an overlay does not discard the underlying new-project draft", (t) => {
  const sheet = mountSheet();
  t.after(() => sheet.stop());
  sheet.state.name = "Draft project";
  const event = escape();
  sheet.document.dispatchEvent(event);
  assert.equal(sheet.closes(), 0);
  assert.equal(event.defaultPrevented, false);
  assert.equal(sheet.state.name, "Draft project");
});

test("the pane owns its Escape key while preserving saving and combobox handling", (t) => {
  const sheet = mountSheet();
  t.after(() => sheet.stop());
  const comboboxEscape = escape();
  comboboxEscape.preventDefault();
  sheet.state.onKeydown(comboboxEscape);
  assert.equal(sheet.closes(), 0);
  sheet.state.saving = true;
  const savingEscape = escape();
  sheet.state.onKeydown(savingEscape);
  assert.equal(sheet.closes(), 0);
  assert.equal(savingEscape.defaultPrevented, true);
  sheet.state.saving = false;
  const paneEscape = escape();
  sheet.state.onKeydown(paneEscape);
  assert.equal(sheet.closes(), 1);
  assert.equal(paneEscape.defaultPrevented, true);
});
