// Compile the actual affected components and bridge; only native services and
// child presentation components are stubbed. A small renderer exposes controls.
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, extname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { compileScript, compileTemplate, parse } from "@vue/compiler-sfc";
import * as vue from "vue";
import ts from "typescript";

const require = createRequire(import.meta.url);
const root = fileURLToPath(new URL("../src/", import.meta.url));
export const settle = () => new Promise<void>(resolve => setImmediate(resolve));
export function deferred<T>() {
  let resolve!: (value: T) => void, reject!: (reason: unknown) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
interface Element { type: string; props: Record<string, any>; children: Element[]; parent?: Element; text?: string; focus?: () => void }
export function mountRoutingComponent(name: string, initial: Record<string, any>, transport: (command: string, args: any) => Promise<any>, renderTemplate = false, savedValues: Record<string, string> = {}, realChildren: string[] = []) {
  const previous = Object.getOwnPropertyDescriptor(globalThis, "localStorage");
  const previousDocument = Object.getOwnPropertyDescriptor(globalThis, "document");
  if (!previousDocument) Object.defineProperty(globalThis, "document", { configurable: true, value: { addEventListener() {}, removeEventListener() {} } });
  const values = new Map<string, string>([["loomUrl", initial.backendUrl ?? "https://a.example"], ...Object.entries(savedValues)]);
  Object.defineProperty(globalThis, "localStorage", { configurable: true, value: {
    getItem: (key: string) => values.get(key) ?? null, setItem: (key: string, value: string) => values.set(key, value), removeItem: (key: string) => values.delete(key),
  } });
  const cache = new Map<string, any>();
  const states: Record<string, any> = {};
  function evaluate(source: string, file: string) {
    const code = ts.transpileModule(source, { compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2021, esModuleInterop: true } }).outputText;
    const module = { exports: {} as any };
    new Function("require", "module", "exports", code)((specifier: string) => {
      if (specifier === "@tauri-apps/api/core") return { invoke: transport };
      if (specifier === "@tauri-apps/api/event") return { listen: async () => () => {} };
      if (specifier === "@tauri-apps/plugin-shell") return { open: async () => {} };
      if (specifier === "dompurify") return { sanitize: (value: string) => value };
      if (specifier === "vue") return { ...vue, withDirectives: (vnode: any) => vnode };
      if (specifier.endsWith("ChatMarkdown.vue")) return { props: ["text"], setup: (props: any) => () => vue.h("span", props.text) };
      if (specifier === "./SplitButton.vue" && renderTemplate) return { __esModule: true, default: compileComponent(resolve(dirname(file), specifier)) };
      if (specifier.endsWith(".vue")) {
        if (realChildren.some(name => specifier.endsWith(`${name}.vue`))) return load(resolve(dirname(file), specifier));
        return { render: () => null };
      }
      if (specifier.startsWith(".")) {
        const next = resolve(dirname(file), specifier);
        return load(extname(next) ? next : `${next}.ts`);
      }
      return require(specifier);
    }, module, module.exports);
    return module.exports;
  }
  function load(file: string): any {
    if (!cache.has(file)) {
      if (file.endsWith(".vue")) {
        const component = compileComponent(file, true);
        const setup = component.setup;
        component.setup = (props: any, context: any) => {
          const state = setup(props, context);
          states[file] = vue.proxyRefs(state);
          return state;
        };
        cache.set(file, { __esModule: true, default: component });
      } else cache.set(file, evaluate(readFileSync(file, "utf8"), file));
    }
    return cache.get(file);
  }
  function compileComponent(file: string, render = renderTemplate) {
    const { descriptor } = parse(readFileSync(file, "utf8"));
    const script = compileScript(descriptor, { id: `routing-${file}` });
    const component = evaluate(script.content, file).default;
    if (render) {
      const template = compileTemplate({ source: descriptor.template!.content, filename: file, id: `routing-${file}`, compilerOptions: { bindingMetadata: script.bindings, expressionPlugins: ["typescript"] } });
      if (template.errors.length) throw new Error(String(template.errors));
      component.render = evaluate(template.code, file).render;
    } else component.render = () => null;
    return component;
  }
  const file = name === "App" ? resolve(root, "App.vue") : resolve(root, "components", `${name}.vue`);
  const component = compileComponent(file, renderTemplate);
  const renderer = vue.createRenderer<Element, Element>({
    createElement: type => ({ type, props: {}, children: [], focus() {}, setSelectionRange() {}, querySelectorAll() { return []; }, getBoundingClientRect() { return { top: 0 }; }, scrollHeight: 0, scrollTop: 0, clientHeight: 0 }),
    createText: text => ({ type: "text", text, props: {}, children: [] }),
    createComment: text => ({ type: "comment", text, props: {}, children: [] }),
    insert(element, parent, anchor) {
      if (element.parent) { const index = element.parent.children.indexOf(element); if (index >= 0) element.parent.children.splice(index, 1); }
      const index = anchor ? parent.children.indexOf(anchor) : -1;
      if (index < 0) parent.children.push(element); else parent.children.splice(index, 0, element);
      element.parent = parent;
    },
    remove(element) { if (element.parent) { const index = element.parent.children.indexOf(element); if (index >= 0) element.parent.children.splice(index, 1); } },
    setText: (element, text) => { element.text = text; },
    setElementText: (element, text) => { element.text = text; element.children = []; },
    patchProp: (element, key, _old, value) => { element.props[key] = value; },
    parentNode: element => element.parent ?? null,
    nextSibling: element => element.parent?.children[element.parent.children.indexOf(element) + 1] ?? null,
  });
  const props = vue.reactive(initial);
  const events: { name: string; args: any[] }[] = [];
  const listeners = Object.fromEntries(["close", "backend", "launch", "created", "layout", "requested", "rename", "save", "setDefault", "error", "select"].map(name => [`on${name[0].toUpperCase()}${name.slice(1)}`, (...args: any[]) => events.push({ name: name === "setDefault" ? "set-default" : name, args })]));
  const app = renderer.createApp({ setup: () => () => vue.h(component, { ...props, ...listeners }) });
  app.provide("backendUrl", vue.computed(() => props.backendUrl));
  const container: Element = { type: "root", props: {}, children: [] };
  const instance = app.mount(container) as any;
  function elements(predicate: (element: Element) => boolean): Element[] {
    const walk = (element: Element): Element[] => [...(predicate(element) ? [element] : []), ...element.children.flatMap(walk)];
    return walk(container);
  }
  return {
    props, events, values, states, state: instance.$.subTree.component.setupState, elements,
    stop() { app.unmount(); if (!previousDocument) Reflect.deleteProperty(globalThis, "document"); if (previous) Object.defineProperty(globalThis, "localStorage", previous); else Reflect.deleteProperty(globalThis, "localStorage"); },
  };
}
