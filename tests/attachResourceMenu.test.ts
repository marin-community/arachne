import assert from "node:assert/strict";
import test from "node:test";
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { compileScript, parse } from "@vue/compiler-sfc";
import { createRenderer, proxyRefs, type Ref, type ComputedRef } from "vue";
import ts from "typescript";

// The one attach control shared by the track panel, project sheet, and
// new-track chat (design.md "Resource attachment"): its menu names what can
// be attached and every choice is a search, not a form. These checks pin the
// draft shapes the three surfaces must handle — search is a mode whose
// chosen entry emits a complete draft.

const require = createRequire(import.meta.url);
const source = readFileSync(new URL("../src/components/AttachResourceMenu.vue", import.meta.url), "utf8");
const script = compileScript(parse(source).descriptor, { id: "attach-menu-test" }).content;
const compiled = ts.transpileModule(script, {
  compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2021, esModuleInterop: true },
}).outputText;

function mountMenu() {
  const previousStorage = Object.getOwnPropertyDescriptor(globalThis, "localStorage");
  const previousDocument = Object.getOwnPropertyDescriptor(globalThis, "document");
  const document = new EventTarget();
  if (!previousDocument) Object.defineProperty(globalThis, "document", { configurable: true, value: document });
  Object.defineProperty(globalThis, "localStorage", { configurable: true, value: {
    getItem: () => null,
    setItem: () => {},
    removeItem: () => {},
  } });
  const module = { exports: {} as any };
  const searchCommands: { command: string; args: any }[] = [];
  new Function("require", "module", "exports", compiled)((name: string) => {
    if (name === "../backendInvoke") return {
      useBackendInvoke: () => async (command: string, args: any) => {
        searchCommands.push({ command, args });
        if (command === "gh_search") return [
          { number: 27, title: "Fix the picker", state: "OPEN", url: "https://github.com/acme/app/issues/27" },
        ];
        if (command === "complete_files") return ["docs/design.md", "docs/other.md"];
        return [];
      },
    };
    if (name === "vue") return require("vue");
    return require(name);
  }, module, module.exports);
  const { default: component } = module.exports;
  const events: { name: string; args: any[] }[] = [];
  const renderer = createRenderer<any, any>({
    createElement: () => ({}), createText: () => ({}), createComment: () => ({}),
    insert() {}, remove() {}, setText() {}, setElementText() {}, patchProp() {},
    parentNode: () => null, nextSibling: () => null,
  });
  return {
    searchCommands,
    events,
    state: (props: Record<string, any> = {}) => {
      events.length = 0;
      const state = proxyRefs(component.setup({ ghRepos: ["acme/app"], repoOptions: ["acme/app"], filesRepo: "acme/app", ...props }, {
        emit: (name: string, ...args: any[]) => events.push({ name, args }),
        expose: () => {},
      })) as any;
      return state;
    },
  };
}

test("the GitHub mode searches and attaches the chosen row's URL", async () => {
  const menu = mountMenu();
  const state = menu.state();
  await Promise.resolve(); // let the repo-option watchers settle
  state.chooseMode("github");
  state.query = "picker";
  await state.runSearch();
  // Debounced search runs after the timer; flush microtasks.
  await new Promise(resolve => setTimeout(resolve, 250));
  assert.equal(menu.searchCommands.at(-1)!.command, "gh_search");
  assert.equal(menu.searchCommands.at(-1)!.args.repo, "acme/app");
  assert.equal(state.ghResults.length, 1);
  state.attachGh(state.ghResults[0]);
  assert.equal(menu.events.length, 1);
  const draft = menu.events[0].args[0];
  assert.equal(draft.kind, "github");
  assert.equal(draft.url, "https://github.com/acme/app/issues/27");
  assert.equal(draft.title, "Fix the picker");
  // The menu closes after an attach — one draft per open.
  assert.equal(state.open, false);
});

test("the file mode completes paths and attaches as the picked kind", async () => {
  const menu = mountMenu();
  const state = menu.state();
  await Promise.resolve();
  state.chooseMode("file", true); // design document
  state.query = "docs/de";
  await state.runSearch();
  await new Promise(resolve => setTimeout(resolve, 250));
  assert.equal(menu.searchCommands.at(-1)!.command, "complete_files");
  assert.equal(state.results.length, 2);
  state.attachPath("docs/design.md");
  const draft = menu.events[0].args[0];
  assert.equal(draft.kind, "design_document");
  assert.equal(draft.path, "docs/design.md");
  assert.equal(draft.title, "design.md");
});

test("the repository mode picks from the managed list", async () => {
  const menu = mountMenu();
  const state = menu.state({ repoOptions: ["acme/app", "acme/lib"] });
  await Promise.resolve();
  state.chooseMode("repository");
  assert.equal(state.pickedRepo, "acme/app");
  state.attachRepo();
  const draft = menu.events[0].args[0];
  assert.equal(draft.kind, "repository");
  assert.equal(draft.repository, "acme/app");
  assert.equal(draft.title, "acme/app");
  // No search runs in repository mode — it is picked, not searched.
  assert.equal(menu.searchCommands.length, 0);
});

test("a menu without repos cannot open a picker that needs one", async () => {
  const menu = mountMenu();
  const state = menu.state({ ghRepos: [], repoOptions: [], filesRepo: null });
  await Promise.resolve();
  state.chooseMode("github");
  state.query = "anything";
  await state.runSearch();
  await new Promise(resolve => setTimeout(resolve, 250));
  assert.equal(menu.searchCommands.length, 0, "no gh_search without a repo to search");
  assert.equal(state.ghResults.length, 0);
});

test("toggle and disabled state", async () => {
  const menu = mountMenu();
  const state = menu.state({ disabled: true });
  state.toggle();
  assert.equal(state.open, false, "disabled menu cannot open");
  const enabled = menu.state();
  enabled.toggle();
  assert.equal(enabled.open, true);
  assert.equal(enabled.mode, "menu", "opens on the kind menu");
  enabled.reset();
  enabled.toggle();
  assert.equal(enabled.open, false);
});
