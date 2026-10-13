import assert from "node:assert/strict";
import test, { type TestContext } from "node:test";
import { nextTick } from "vue";
import { scopedId } from "../src/backendIdentity.ts";
import { registeredBackends } from "../src/backendFleet.ts";
import { mountRoutingComponent, deferred, settle } from "./componentRoutingHarness.ts";

const local = "http://127.0.0.1:7878";
const remote = "https://remote.example";
const live = "https://live.example";
const contents = (el: any): string => (el.text ?? "") + el.children.map(contents).join("");
const backend = (url: string, name: string) => ({ url, name, available: true, connecting: false, error: null, sessions: [], layout: null, launchOptions: null });
function settings(t: TestContext, transport: (command: string, args: any) => Promise<any> = async () => null) {
  const previous = Object.getOwnPropertyDescriptor(globalThis, "document");
  Object.defineProperty(globalThis, "document", { configurable: true, value: { addEventListener() {}, removeEventListener() {} } });
  t.mock.timers.enable({ apis: ["setTimeout"] });
  const sheet = mountRoutingComponent("SettingsSheet", { url: remote, token: "", connected: true, saving: false, error: null, backends: [backend(remote, "Remote"), backend(live, "Live only")] }, transport, true,
    { loomUrl: remote, "arachne.loomServers": JSON.stringify([{ name: "Remote", url: remote }, { name: "Saved only", url: "https://saved.example" }]) });
  let stopped = false;
  const stop = () => { if (!stopped) { stopped = true; sheet.stop(); } };
  t.after(() => { stop(); if (previous) Object.defineProperty(globalThis, "document", previous); else Reflect.deleteProperty(globalThis, "document"); });
  return { ...sheet, stop };
}
const button = (sheet: ReturnType<typeof settings>, label: string) => sheet.elements(el => el.type === "button" && el.text === label)[0];
async function select(t: TestContext, sheet: ReturnType<typeof settings>, url: string) {
  sheet.elements(el => el.type === "select")[0].props.onChange({ target: { value: url } });
  await nextTick(); t.mock.timers.tick(200); await settle();
}

test("saved, live-only and implicit local servers are selectable; local rename persists with retry", async t => {
  const calls: any[] = [];
  const sheet = settings(t, async (command, args) => { calls.push({ command, args }); return `token:${args.baseUrl}`; });
  await settle();
  const options = sheet.elements(el => el.type === "option").map(el => [el.props.value, contents(el)]);
  for (const url of [remote, live, local, "https://saved.example"]) assert.ok(options.some(row => row[0] === url));
  await select(t, sheet, live);
  assert.equal(sheet.state.name, "Live only"); assert.equal(sheet.state.token, `token:${live}`);
  await select(t, sheet, "https://saved.example"); assert.equal(sheet.state.name, "Saved only");
  await select(t, sheet, local);
  assert.equal(Boolean(button(sheet, "Set default").props.disabled), true, "register an implicit endpoint first");
  sheet.state.name = "local"; await nextTick();
  assert.equal(Boolean(button(sheet, "Rename server").props.disabled), false);
  const storage = localStorage, save = storage.setItem;
  const before = calls.length;
  storage.setItem = () => { throw new Error("storage unavailable"); };
  button(sheet, "Rename server").props.onClick(); await nextTick();
  assert.ok(sheet.elements(el => el.props.role === "alert" && contents(el).includes("storage unavailable")).length);
  assert.equal(sheet.state.name, "local"); assert.equal(Boolean(button(sheet, "Rename server").props.disabled), false);
  assert.equal(sheet.state.profiles.some((p: any) => p.url === local), false); assert.deepEqual(sheet.events, []);
  storage.setItem = save;
  button(sheet, "Rename server").props.onClick(); await nextTick();
  assert.deepEqual(sheet.events, [{ name: "rename", args: [local, "local"] }]);
  assert.equal(calls.length, before, "rename has no credential or connection operations");
  assert.equal(sheet.state.token, `token:${local}`); assert.equal(sheet.values.get("loomUrl"), remote);
  assert.equal(sheet.elements(el => el.props.role === "alert").length, 0);
  assert.equal(registeredBackends(localStorage, remote).find(p => p.url === local)?.name, "local", "alias survives restart");
});

test("save configuration persists first, retries storage failure and leaves default selection explicit", async t => {
  const sheet = settings(t, async () => "saved-token"); await settle();
  await select(t, sheet, live);
  sheet.state.name = "Renamed live"; await nextTick();
  assert.equal(Boolean(button(sheet, "Set default").props.disabled), true, "save edited config first");
  const storage = localStorage, save = storage.setItem;
  storage.setItem = () => { throw new Error("quota exceeded"); };
  sheet.elements(el => el.type === "form")[0].props.onSubmit({ preventDefault() {} }); await nextTick();
  assert.deepEqual(sheet.events, []); assert.equal(sheet.state.name, "Renamed live");
  assert.ok(sheet.elements(el => el.props.role === "alert" && contents(el).includes("Could not save server configuration: quota exceeded")).length);
  storage.setItem = save;
  sheet.elements(el => el.type === "form")[0].props.onSubmit({ preventDefault() {} }); await nextTick();
  assert.deepEqual(sheet.events, [{ name: "save", args: [live, undefined, "Renamed live"] }], "unchanged credential is omitted");
  assert.equal(sheet.values.get("loomUrl"), remote); assert.equal(sheet.state.localError, "");
  assert.equal(Boolean(button(sheet, "Set default").props.disabled), false);
  button(sheet, "Set default").props.onClick();
  assert.deepEqual(sheet.events.at(-1), { name: "set-default", args: [live] });
});

test("editing the address adds an endpoint and cannot copy the previous server's credential", async t => {
  const sheet = settings(t, async (_command, args) => args.baseUrl === remote ? "remote-secret" : null); await settle();
  const target = "https://new.example";
  sheet.state.url = target; await nextTick();
  assert.equal(sheet.state.token, ""); assert.equal(sheet.state.tokenLoading, true);
  assert.equal(Boolean(button(sheet, "Set default").props.disabled), true);
  assert.ok(sheet.elements(el => el.props.class === "hint" && contents(el).includes("original server and its projects stay")).length);
  t.mock.timers.tick(200); await settle();
  sheet.state.name = "New endpoint"; sheet.state.token = "new-secret"; await nextTick();
  sheet.elements(el => el.type === "form")[0].props.onSubmit({ preventDefault() {} }); await nextTick();
  assert.deepEqual(sheet.events, [{ name: "save", args: [target, "new-secret", "New endpoint"] }]);
  const profiles = JSON.parse(sheet.values.get("arachne.loomServers")!);
  assert.ok(profiles.some((p: any) => p.url === remote && p.name === "Remote"));
  assert.ok(profiles.some((p: any) => p.url === target && p.name === "New endpoint"));
  assert.equal(sheet.values.get("loomUrl"), remote); assert.equal(JSON.stringify([...sheet.values]).includes("secret"), false);
  assert.equal(Boolean(button(sheet, "Set default").props.disabled), true, "wait for credential save success");
  sheet.props.backends.push(backend(target, "New endpoint"));
  sheet.props.saving = true; await nextTick();
  sheet.props.error = "Keychain write failed"; sheet.props.saving = false; await nextTick();
  assert.equal(Boolean(button(sheet, "Set default").props.disabled), true, "failed save leaves credential edit retryable");
  sheet.elements(el => el.type === "form")[0].props.onSubmit({ preventDefault() {} });
  assert.deepEqual(sheet.events.at(-1), { name: "save", args: [target, "new-secret", "New endpoint"] });
  sheet.props.error = null; sheet.props.saving = true; await nextTick(); sheet.props.saving = false; await nextTick();
  assert.equal(Boolean(button(sheet, "Set default").props.disabled), false);
});

for (const outcome of ["success", "failure"] as const) test(`late Keychain ${outcome} cannot overwrite selected endpoint or unmounted settings`, async t => {
  const old = deferred<string | null>(), current = deferred<string | null>(), last = deferred<string | null>();
  const sheet = settings(t, async (_command, args) => args.baseUrl === remote ? old.promise : args.baseUrl === live ? current.promise : last.promise);
  await select(t, sheet, live); current.resolve("live-secret"); await settle();
  if (outcome === "success") old.resolve("remote-secret"); else old.reject(new Error("remote keychain failure"));
  await settle(); assert.equal(sheet.state.token, "live-secret"); assert.equal(sheet.state.localError, ""); assert.equal(sheet.state.tokenLoading, false);
  await select(t, sheet, local); sheet.stop();
  const state = [sheet.state.token, sheet.state.localError, sheet.state.tokenLoading];
  if (outcome === "success") last.resolve("late-secret"); else last.reject(new Error("late failure"));
  await settle(); assert.deepEqual([sheet.state.token, sheet.state.localError, sheet.state.tokenLoading], state);
});

test("footer text and title show raw session identity for colliding scoped IDs", async t => {
  const id = "chat-1234";
  const sheet = mountRoutingComponent("ContextTelemetry", { sessionId: scopedId(remote, id), usage: null }, async () => null, true); t.after(sheet.stop);
  const footer = () => sheet.elements(el => el.props.class === "session-id")[0];
  for (const url of [remote, live]) {
    sheet.props.sessionId = scopedId(url, id); await nextTick();
    assert.equal(contents(footer()), `⌘ ${id}`); assert.equal(footer().props.title, id);
    assert.equal(sheet.props.sessionId, scopedId(url, id), "display leaves routing identity untouched");
  }
  sheet.props.sessionId = "raw-chat"; await nextTick();
  assert.equal(contents(footer()), "⌘ raw-chat"); assert.equal(footer().props.title, "raw-chat");
});
