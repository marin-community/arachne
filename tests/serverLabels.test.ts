import assert from "node:assert/strict";
import test from "node:test";
import { nextTick } from "vue";
import { scopeSession, scopeLayout, scopedId } from "../src/backendIdentity.ts";
import { mountRoutingComponent, settle } from "./componentRoutingHarness.ts";

const url = "http://127.0.0.1:9876";
const backend = () => ({ url, name: "127.0.0.1:9876", available: true, connecting: false, error: null, sessions: [], layout: null, launchOptions: null });
const session = (id: string, parent: string | null = null) => scopeSession(url, {
  id, status: "idle", profile: "default", class: "interactive", origin: "user", parent_id: null, parent_session_id: parent,
  last_activity_at: "2026-10-11T00:00:00Z", placement: { group_id: "project", space_id: "space", session_id: id },
  branch: { id: `branch-${id}`, branch: `weaver/${id}`, name: id, title: id, tags: [{ key: "topic", value: "true" }] },
} as any);
const layout = scopeLayout(url, { revision: 1, defaults: [], spaces: [{ id: "space", name: "User", rank: 0, system_key: "user", groups: [
  { id: "project", space_id: "space", name: "Project", rank: 0, system_key: null, collapsed: false, session_ids: ["track", "chat"] },
] }] });
const contents = (el: any): string => (el.text ?? "") + el.children.map(contents).join("");

test("project headers keep live aliases while track and chat rows omit server labels", async t => {
  const sidebar = mountRoutingComponent("FleetSidebar", { fleet: [session("track"), { ...session("standalone"), branch: { ...session("standalone").branch, tags: [{ key: "topic", value: "false" }] } }, session("chat", "standalone")], backends: [backend()], layout, selectedId: null, unarchivingIds: new Set() }, async () => null, true);
  t.after(sidebar.stop); await nextTick();
  const tracks = sidebar.elements(el => String(el.props.class).includes("topic-card"));
  assert.equal(tracks.length, 1);
  assert.equal(contents(tracks[0]).includes("127.0.0.1:9876"), false);
  assert.equal(sidebar.elements(el => el.props.class === "project-server")[0].text, "127.0.0.1:9876");
  sidebar.props.backends[0].name = "Research"; await nextTick();
  assert.equal(sidebar.elements(el => el.props.class === "project-server")[0].text, "Research");
  sidebar.state.tab = "inbox"; await nextTick();
  const chats = sidebar.elements(el => String(el.props.class).includes("session-item"));
  assert.equal(chats.length, 2);
  for (const row of chats) {
    assert.equal(contents(row).includes("127.0.0.1:9876"), false);
    assert.equal(contents(row).includes("Research"), false);
  }
  assert.equal(sidebar.state.serverLabel(scopedId(url, "project")), "Research");
});

test("dedicated Rename server button persists alias and emits only rename", async t => {
  const previous = Object.getOwnPropertyDescriptor(globalThis, "document");
  Object.defineProperty(globalThis, "document", { configurable: true, value: { addEventListener() {}, removeEventListener() {} } });
  const calls: string[] = [];
  const sheet = mountRoutingComponent("SettingsSheet", { url, token: "", connected: true, backends: [backend()] }, async command => { calls.push(command); return "saved-secret"; }, true,
    { "loomUrl": url, "arachne.loomServers": JSON.stringify([{ name: "Old name", url }]) });
  t.after(() => { sheet.stop(); if (previous) Object.defineProperty(globalThis, "document", previous); else Reflect.deleteProperty(globalThis, "document"); }); await settle();
  sheet.state.name = "  Research  "; await nextTick();
  const rename = sheet.elements(el => el.type === "button" && el.text === "Rename server")[0];
  assert.ok(rename, "dedicated visible action"); assert.equal(rename.props.type, "button"); assert.equal(Boolean(rename.props.disabled), false);
  const before = calls.length; rename.props.onClick(); await nextTick();
  assert.deepEqual(sheet.events, [{ name: "rename", args: [url, "Research"] }]);
  assert.equal(calls.length, before, "name-only update never touches Keychain/connection");
  assert.equal(sheet.state.url, url); assert.equal(sheet.state.token, "saved-secret");
  assert.equal(sheet.values.get("loomUrl"), url);
  assert.deepEqual(JSON.parse(sheet.values.get("arachne.loomServers")!), [{ name: "Research", url }]);
  assert.equal(JSON.stringify([...sheet.values]).includes("saved-secret"), false);
  assert.ok(sheet.elements(el => el.type === "option" && contents(el).startsWith("Research ·")).length);
  assert.equal(Boolean(sheet.elements(el => el.type === "button" && el.text === "Rename server")[0].props.disabled), true);
});

test("backend choices update live from renamed profiles", async t => {
  const sheet = mountRoutingComponent("ProjectSheet", { backendUrl: url, backends: [backend()], available: true }, async () => [], true);
  t.after(sheet.stop); await settle();
  sheet.props.backends[0].name = "Research"; await nextTick();
  assert.ok(sheet.elements(el => el.type === "option" && contents(el).startsWith("Research ·")).length);
  assert.equal(sheet.props.backendUrl, url);
  assert.equal(sheet.events.length, 0);
});


test("a failed rename shows its persistence error and keeps the edit retryable", async t => {
  const previous = Object.getOwnPropertyDescriptor(globalThis, "document");
  Object.defineProperty(globalThis, "document", { configurable: true, value: { addEventListener() {}, removeEventListener() {} } });
  const calls: string[] = [];
  const original = { ...backend(), name: "Original" };
  const sheet = mountRoutingComponent("SettingsSheet", { url, token: "", connected: true, backends: [original] }, async command => { calls.push(command); return "saved-secret"; }, true,
    { "loomUrl": url, "arachne.loomServers": JSON.stringify([{ name: "Original", url }]) });
  t.after(() => { sheet.stop(); if (previous) Object.defineProperty(globalThis, "document", previous); else Reflect.deleteProperty(globalThis, "document"); }); await settle();
  const storage = localStorage;
  const save = storage.setItem;
  storage.setItem = () => { throw new DOMException("Storage quota exceeded", "QuotaExceededError"); };
  sheet.state.name = "Research"; await nextTick();
  const rename = () => sheet.elements(el => el.type === "button" && el.text === "Rename server")[0];
  const before = calls.length;
  rename().props.onClick(); await nextTick();
  assert.deepEqual(JSON.parse(sheet.values.get("arachne.loomServers")!), [{ name: "Original", url }]);
  assert.equal(sheet.state.profiles.find((profile: any) => profile.url === url).name, "Original");
  assert.equal(sheet.props.backends[0].name, "Original", "failed save cannot update the live backend alias");
  assert.equal(sheet.state.name, "Research", "keep the user's edit for retry");
  assert.ok(sheet.elements(el => el.props.role === "alert" && contents(el).includes("Could not save server name: Storage quota exceeded")).length, "show a visible persistence failure");
  assert.equal(Boolean(rename().props.disabled), false, "rename remains retryable");
  assert.deepEqual(sheet.events, [], "failed persistence cannot emit a rename or connection action");
  assert.equal(calls.length, before);
  assert.equal(sheet.state.url, url); assert.equal(sheet.state.token, "saved-secret"); assert.equal(sheet.values.get("loomUrl"), url);
  storage.setItem = save;
  rename().props.onClick(); await nextTick();
  assert.deepEqual(JSON.parse(sheet.values.get("arachne.loomServers")!), [{ name: "Research", url }]);
  assert.equal(sheet.state.profiles.find((profile: any) => profile.url === url).name, "Research");
  assert.deepEqual(sheet.events, [{ name: "rename", args: [url, "Research"] }]);
  assert.equal(sheet.state.localError, "");
  assert.equal(sheet.elements(el => el.props.role === "alert").length, 0, "successful retry clears the error");
  assert.equal(Boolean(rename().props.disabled), true);
  assert.equal(calls.length, before); assert.equal(sheet.state.url, url); assert.equal(sheet.state.token, "saved-secret"); assert.equal(sheet.values.get("loomUrl"), url);
});


test("rename persistence errors take precedence over an earlier connection error and allow retry", async t => {
  const previous = Object.getOwnPropertyDescriptor(globalThis, "document");
  Object.defineProperty(globalThis, "document", { configurable: true, value: { addEventListener() {}, removeEventListener() {} } });
  const calls: string[] = [];
  const priorError = "Previous connection failed";
  const sheet = mountRoutingComponent("SettingsSheet", { url, token: "", connected: true, error: priorError, backends: [{ ...backend(), name: "Original" }] }, async command => { calls.push(command); return "saved-secret"; }, true,
    { "loomUrl": url, "arachne.loomServers": JSON.stringify([{ name: "Original", url }]) });
  t.after(() => { sheet.stop(); if (previous) Object.defineProperty(globalThis, "document", previous); else Reflect.deleteProperty(globalThis, "document"); }); await settle();
  const alerts = () => sheet.elements(el => el.props.role === "alert").map(contents);
  assert.deepEqual(alerts(), [priorError]);
  const storage = localStorage;
  const save = storage.setItem;
  storage.setItem = () => { throw new DOMException("alias storage denied", "QuotaExceededError"); };
  sheet.state.name = "Research"; await nextTick();
  const rename = () => sheet.elements(el => el.type === "button" && el.text === "Rename server")[0];
  const before = calls.length;
  rename().props.onClick(); await nextTick();
  assert.deepEqual(alerts(), ["Could not save server name: alias storage denied"], "current rename failure must be visible despite the earlier connection error");
  assert.deepEqual(JSON.parse(sheet.values.get("arachne.loomServers")!), [{ name: "Original", url }]);
  assert.equal(sheet.state.profiles.find((profile: any) => profile.url === url).name, "Original");
  assert.equal(sheet.props.backends[0].name, "Original");
  assert.equal(sheet.state.name, "Research");
  assert.equal(Boolean(rename().props.disabled), false);
  assert.deepEqual(sheet.events, []);
  assert.equal(calls.length, before);
  storage.setItem = save;
  rename().props.onClick(); await nextTick();
  assert.deepEqual(JSON.parse(sheet.values.get("arachne.loomServers")!), [{ name: "Research", url }]);
  assert.deepEqual(sheet.events, [{ name: "rename", args: [url, "Research"] }]);
  assert.equal(sheet.state.localError, "");
  assert.deepEqual(alerts(), [priorError], "clearing the local failure preserves the parent's connection error");
  assert.equal(Boolean(rename().props.disabled), true);
  assert.equal(calls.length, before);
  assert.equal(sheet.state.url, url); assert.equal(sheet.state.token, "saved-secret"); assert.equal(sheet.values.get("loomUrl"), url);
  assert.equal(sheet.props.error, priorError);
});
