import assert from "node:assert/strict";
import test from "node:test";
import { scopedId, splitScopedId, backendOf, scopeSession, scopeLayout } from "../src/backendIdentity.ts";
import { registeredBackends, acceptSnapshot, markUnavailable, combinedFleet, combinedLayout, cacheProjectLayout, PROJECT_CACHE_KEY } from "../src/backendFleet.ts";
import { rememberServer } from "../src/serverProfiles.ts";

const a = "https://a.example";
const b = "https://b.example";
function storage() {
  const values = new Map<string, string>();
  return { getItem: (key: string) => values.get(key) ?? null, setItem: (key: string, value: string) => { values.set(key, value); } };
}
function layout(name = "Project") {
  return { revision: 3, spaces: [{ id: "space", name: "User", rank: 0, system_key: "user", groups: [
    { id: "group", space_id: "space", name, rank: 0, system_key: null, collapsed: false, session_ids: ["session"] },
  ] }], defaults: [{ selector_kind: "profile", selector_value: "default", group_id: "group" }] };
}
function session() {
  return { id: "session", branch: { id: "branch" }, parent_id: "parent-branch", parent_session_id: "parent-session",
    placement: { space_id: "space", group_id: "group", session_id: "session" } } as any;
}

test("composite identities preserve local IDs and prevent cross-backend collisions", () => {
  const local = "colon: slash/ % snowman☃";
  assert.notEqual(scopedId(a, local), scopedId(b, local));
  assert.deepEqual(splitScopedId(scopedId(a + "/", local)), { baseUrl: a, id: local });
  assert.equal(backendOf("legacy-id"), null);
  assert.throws(() => scopedId("https://user:secret@a.example", local));
  const scoped = scopeSession(a, session());
  assert.equal(scoped.placement!.group_id, scopedId(a, "group"));
  assert.equal(scoped.parent_id, scopedId(a, "parent-branch"));
  assert.equal(scoped.parent_session_id, scopedId(a, "parent-session"));
  assert.equal(scoped.branch.id, scopedId(a, "branch"));
  assert.equal(session().id, "session", "wire objects stay unchanged");
  assert.equal(scopeLayout(b, layout()).spaces[0].groups[0].session_ids[0], scopedId(b, "session"));
});

test("migrate the legacy server alongside remembered backends and restore offline project headings", () => {
  const local = storage();
  local.setItem("arachne.loomServers", JSON.stringify([{ name: "B", url: b }]));
  cacheProjectLayout(local, a, layout("Original project"));
  const backends = registeredBackends(local, a + "/");
  assert.deepEqual(backends.map(backend => backend.url), [b, a]);
  assert.deepEqual(registeredBackends(local, b).map(backend => backend.url).sort(), [a, b]);
  assert.equal(backends[1].layout!.spaces[0].groups[0].name, "Original project");
  assert.equal(backends[1].layout!.spaces[0].groups[0].id, scopedId(a, "group"));
  assert.equal(backends[1].available, false);
  assert.deepEqual(backends[1].sessions, []);
  const persisted = JSON.parse(local.getItem(PROJECT_CACHE_KEY)!);
  assert.deepEqual(persisted[a].spaces[0].groups[0].session_ids, []);
  assert.equal(local.getItem("loomToken"), null);
});

test("corrupt project cache never prevents backend registration or later persistence", () => {
  for (const invalid of ["null", "[]", "not json", JSON.stringify({ [a]: { spaces: [{}] } })]) {
    const local = storage();
    local.setItem(PROJECT_CACHE_KEY, invalid);
    assert.equal(registeredBackends(local, a)[0].layout, null);
    cacheProjectLayout(local, a, layout());
    // An unparsable cache is a best-effort write; registration still works.
    assert.equal(registeredBackends(local, a)[0].url, a);
  }
});

test("registering additional backends never silently evicts an existing project backend", () => {
  const local = storage();
  registeredBackends(local, a);
  for (let index = 0; index < 14; index++) rememberServer(local, { name: `Backend ${index}`, url: `https://backend-${index}.example` });
  const restored = registeredBackends(local, "https://backend-13.example");
  assert.equal(restored.length, 15);
  assert.ok(restored.some(backend => backend.url === a));
});

test("an outage retains only that backend's headings; healthy tracks and recovery keep identities", () => {
  const local = storage();
  local.setItem("arachne.loomServers", JSON.stringify([{ name: "B", url: b }]));
  const backends = registeredBackends(local, a);
  for (const backend of backends) acceptSnapshot(backend, [session()], layout());
  assert.equal(combinedFleet(backends).length, 2);
  assert.equal(new Set(combinedFleet(backends).map(row => row.id)).size, 2);
  const healthySession = backends[0].sessions[0];
  markUnavailable(backends[1], "Connection refused");
  assert.deepEqual(combinedFleet(backends).map(row => row.id), [scopedId(b, "session")]);
  assert.equal(backends[0].sessions[0], healthySession);
  assert.equal(combinedLayout(backends).spaces.length, 2, "unavailable projects retain headings");
  assert.equal(backends[1].error, "Connection refused");
  acceptSnapshot(backends[1], [session()], layout("Recovered project"));
  assert.equal(combinedFleet(backends).length, 2);
  assert.equal(backends[1].sessions[0].id, scopedId(a, "session"));
  assert.equal(backends[1].error, null);
  assert.equal(backends[0].sessions[0], healthySession, "recovery does not replace healthy fleet");
});
