import assert from "node:assert/strict";
import test, { type TestContext } from "node:test";
import { nextTick } from "vue";
import { mountRoutingComponent, deferred, settle } from "./componentRoutingHarness.ts";

const a = "https://a.example", b = "https://b.example";
const contents = (el: any): string => (el.text ?? "") + el.children.map(contents).join("");
async function mounted(t: TestContext, intercept?: (command: string, args: any) => Promise<any> | undefined) {
  const previous = new Map(["window", "document"].map(key => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  for (const key of previous.keys()) Object.defineProperty(globalThis, key, { configurable: true, value: new EventTarget() });
  t.mock.timers.enable({ apis: ["setTimeout"] });
  const calls: { command: string; args: any }[] = [];
  const app = mountRoutingComponent("App", {}, async (command, args) => {
    calls.push({ command, args });
    const result = intercept?.(command, args);
    if (result !== undefined) return result;
    if (command === "refresh_fleet") return { server_url: args.baseUrl, sessions: [], layout: { revision: 0, defaults: [], spaces: [] } };
    if (command === "launch_options") return { agents: [], profiles: [], default_agent: "" };
    return null;
  }, true, { loomUrl: a, "arachne.loomServers": JSON.stringify([{ name: "A", url: a }, { name: "B", url: b }]) }, ["SettingsSheet"]);
  t.after(() => {
    app.stop();
    for (const [key, descriptor] of previous) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor); else Reflect.deleteProperty(globalThis, key);
    }
  });
  await settle();
  app.elements(el => el.type === "button" && el.props.title === "Connection settings")[0].props.onClick();
  await settle();
  const sheet = Object.entries(app.states).find(([file]) => file.endsWith("/SettingsSheet.vue"))![1];
  const button = (label: string) => app.elements(el => el.type === "button" && contents(el) === label)[0];
  const alerts = () => app.elements(el => el.props.role === "alert").map(contents);
  const submit = async () => { app.elements(el => el.type === "form")[0].props.onSubmit({ preventDefault() {} }); await settle(); };
  const choose = async (url: string) => {
    app.elements(el => el.type === "select")[0].props.onChange({ target: { value: url } });
    await nextTick(); t.mock.timers.tick(200); await settle();
  };
  const address = async (url: string) => {
    app.elements(el => el.type === "input" && el.props.placeholder === "http://127.0.0.1:7878")[0].props["onUpdate:modelValue"](url);
    await nextTick(); t.mock.timers.tick(200); await settle();
  };
  return { ...app, calls, sheet, button, alerts, submit, choose, address };
}

for (const stored of [null, "stored-token"]) test(`failed new endpoint retries unchanged ${stored ? "stored" : "empty"} credential on Save`, async t => {
  const target = "http://127.0.0.1:9898";
  let down = true;
  const app = await mounted(t, (command, args) => {
    if (command === "load_token" && args.baseUrl === target) return Promise.resolve(stored);
    if (command === "connect" && args.baseUrl === target && down) return Promise.reject(new Error("temporary connection failure"));
  });
  await app.address(target); await app.submit();
  assert.equal(app.state.settingsError, "temporary connection failure");
  assert.ok(app.alerts().some(text => text.includes("temporary connection failure")));
  const connections = () => app.calls.filter(call => call.command === "connect" && call.args.baseUrl === target);
  assert.equal(connections().length, 1);
  await app.submit(); assert.equal(connections().length, 2, "failed retry must reconnect and keep its failure visible");
  assert.equal(app.state.settingsError, "temporary connection failure");
  down = false; await app.submit();
  assert.equal(connections().length, 3, "retry must attempt native connection after endpoint registration");
  assert.equal(connections().at(-1)!.args.token, stored);
  assert.equal(app.state.backends.find((backend: any) => backend.url === target).available, true);
  assert.equal(app.state.settingsError, null); assert.deepEqual(app.alerts(), []);
  assert.equal(app.values.get("loomUrl"), a);
  const beforeRename = app.calls.length;
  app.sheet.name = "Healthy alias"; await nextTick(); await app.submit();
  assert.equal(app.calls.length, beforeRename, "healthy name-only config save must have zero native calls");
});

test("unavailable existing backend reconnects on config Save while healthy metadata saves remain independent", async t => {
  const app = await mounted(t);
  await app.choose(b);
  app.state.backends.find((backend: any) => backend.url === b).available = false;
  const before = app.calls.filter(call => call.command === "connect" && call.args.baseUrl === b).length;
  await app.submit();
  assert.equal(app.calls.filter(call => call.command === "connect" && call.args.baseUrl === b).length, before + 1);
  assert.equal(app.state.backends.find((backend: any) => backend.url === b).available, true);
  assert.equal(app.values.get("loomUrl"), a);
});

test("failed token read stays unknown and visible across metadata rename/save until explicit retry succeeds", async t => {
  let deny = false;
  const app = await mounted(t, (command, args) => command === "load_token" && args.baseUrl === b
    ? deny ? Promise.reject(new Error("Keychain read unavailable")) : Promise.resolve("stored-b-token") : undefined);
  deny = true; await app.choose(b);
  assert.ok(app.alerts().some(text => text.includes("Keychain read unavailable")));
  const before = app.calls.length;
  app.sheet.name = "B alias"; await nextTick(); app.button("Rename server").props.onClick(); await settle();
  assert.ok(app.alerts().some(text => text.includes("Keychain read unavailable")), "rename cannot clear unresolved read errors");
  await app.submit();
  assert.ok(app.alerts().some(text => text.includes("Keychain read unavailable")), "metadata Save cannot mask unresolved read errors");
  assert.equal(app.sheet.loadedToken, null, "read failure is distinct from a known empty token");
  assert.equal(app.calls.length, before, "healthy metadata save must not perform credential operations");
  const retry = app.button("Retry token read"); assert.ok(retry, "offer an explicit read retry");
  retry.props.onClick(); await settle();
  assert.ok(app.alerts().some(text => text.includes("Keychain read unavailable")), "failed retry remains visible");
  assert.equal(app.sheet.loadedToken, null);
  deny = false; app.button("Retry token read").props.onClick(); await settle();
  assert.equal(app.sheet.loadedToken, "stored-b-token"); assert.equal(app.sheet.token, "stored-b-token");
  assert.deepEqual(app.alerts(), []);
  assert.equal(app.calls.at(-1)!.command, "load_token"); assert.equal(app.calls.at(-1)!.args.baseUrl, b);
});

for (const outcome of ["success", "error"] as const) test(`stale token read retry ${outcome} cannot overwrite a newly selected server`, async t => {
  const late = deferred<string | null>();
  let deny = true, retrying = false;
  const app = await mounted(t, (command, args) => {
    if (command !== "load_token") return;
    if (args.baseUrl === b) return retrying ? late.promise : deny ? Promise.reject(new Error("read denied")) : Promise.resolve(null);
    return Promise.resolve("a-token");
  });
  await app.choose(b); retrying = true;
  const retry = app.button("Retry token read"); assert.ok(retry); retry.props.onClick(); await settle();
  assert.equal(app.sheet.tokenLoading, true);
  deny = false; await app.choose(a);
  assert.equal(app.sheet.token, "a-token"); assert.equal(app.sheet.loadedToken, "a-token");
  if (outcome === "success") late.resolve("stale-b-token"); else late.reject(new Error("stale retry failure"));
  await settle();
  assert.equal(app.sheet.token, "a-token"); assert.equal(app.sheet.loadedToken, "a-token");
  assert.equal(app.sheet.tokenLoading, false); assert.deepEqual(app.alerts(), []);
});

test("credential write failure keeps the edited credential retryable without claiming a successful read", async t => {
  let denyRead = false, denyWrite = true;
  const app = await mounted(t, (command, args) => {
    if (command === "load_token" && args.baseUrl === b && denyRead) return Promise.reject(new Error("read unavailable"));
    if (command === "save_token" && args.baseUrl === b && denyWrite) return Promise.reject(new Error("write unavailable"));
  });
  denyRead = true; await app.choose(b);
  app.sheet.token = "replacement-b-token"; await nextTick(); await app.submit();
  assert.equal(app.state.settingsError, "write unavailable");
  assert.equal(app.sheet.loadedToken, null); assert.equal(app.sheet.token, "replacement-b-token");
  assert.ok(app.alerts().some(text => text.includes("read unavailable")));
  assert.ok(app.alerts().some(text => text.includes("write unavailable")));
  denyWrite = false; await app.submit();
  const writes = app.calls.filter(call => call.command === "save_token");
  assert.equal(writes.length, 2); assert.ok(writes.every(call => call.args.baseUrl === b && call.args.token === "replacement-b-token"));
  assert.equal(app.sheet.loadedToken, "replacement-b-token"); assert.deepEqual(app.alerts(), []);
  assert.equal(app.values.get("loomUrl"), a); assert.equal(JSON.stringify([...app.values]).includes("replacement-b-token"), false);
  const before = app.calls.length; app.sheet.name = "B renamed"; await nextTick(); await app.submit();
  assert.equal(app.calls.length, before, "healthy metadata-only save after write recovery is independent");
});
