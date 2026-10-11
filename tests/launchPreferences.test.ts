import assert from "node:assert/strict";
import test from "node:test";
import { readProjectDefaults, saveProjectDefaults, recentRepositories, rememberRepository } from "../src/launchDefaults.ts";
import { normalizeServerUrl, readServerProfiles, rememberServer, SERVER_PROFILES_KEY } from "../src/serverProfiles.ts";

function storage() {
  const data = new Map<string, string>();
  return { getItem: (key: string) => data.get(key) ?? null, setItem: (key: string, value: string) => { data.set(key, value); } };
}

test("launch defaults and recent repositories stay isolated by Loom server", () => {
  const s = storage();
  s.setItem("loomUrl", "http://localhost:7878");
  saveProjectDefaults(s, "project-1", { repo: "org/alpha", base: "main" });
  rememberRepository(s, "org/beta");
  rememberRepository(s, "org/alpha");
  assert.deepEqual(recentRepositories(s), ["org/alpha", "org/beta"]);
  s.setItem("loomUrl", "http://localhost:7879");
  assert.equal(readProjectDefaults(s, "project-1"), null);
  assert.deepEqual(recentRepositories(s), []);
  s.setItem("loomUrl", "http://localhost:7878/");
  assert.deepEqual(readProjectDefaults(s, "project-1"), { repo: "org/alpha", base: "main" });
});

test("saved server profiles contain only validated names and URLs", () => {
  const s = storage();
  assert.equal(normalizeServerUrl(" HTTP://LOCALHOST:80/ "), "http://localhost");
  for (const url of ["file:///tmp/loom", "http://user:secret@localhost", "https://host?token=secret", "https://host/#secret"]) {
    assert.throws(() => normalizeServerUrl(url));
  }
  rememberServer(s, { name: "Local", url: "http://localhost:7878/" });
  rememberServer(s, { name: "Renamed", url: "http://localhost:7878" });
  assert.deepEqual(readServerProfiles(s), [{ name: "Renamed", url: "http://localhost:7878" }]);
  s.setItem(SERVER_PROFILES_KEY, JSON.stringify([{ name: "Bad", url: "https://secret:token@host" }, { name: "Good", url: "https://host", token: "never copied" }]));
  assert.deepEqual(readServerProfiles(s), [{ name: "Good", url: "https://host" }]);
});

test("explicit backend preferences ignore the legacy selected server and decode old project keys", async () => {
  const { scopedId } = await import("../src/backendIdentity.ts");
  const s = storage();
  s.setItem("loomUrl", "http://localhost:7878");
  saveProjectDefaults(s, "old-project", { repo: "org/old", base: "main" });
  const project = scopedId("http://localhost:7878", "old-project");
  s.setItem("loomUrl", "http://localhost:7879");
  assert.deepEqual(readProjectDefaults(s, project), { repo: "org/old", base: "main" });
  saveProjectDefaults(s, project, { repo: "org/new", base: "dev" });
  assert.deepEqual(readProjectDefaults(s, project), { repo: "org/new", base: "dev" });
  assert.deepEqual(recentRepositories(s, "http://localhost:7878/"), ["org/new", "org/old"]);
  assert.deepEqual(recentRepositories(s), []);
  rememberRepository(s, "org/other", "http://localhost:7879");
  assert.deepEqual(recentRepositories(s), ["org/other"]);
});
