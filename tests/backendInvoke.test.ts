import assert from "node:assert/strict";
import test from "node:test";
import { createBackendInvoke, resolveBackendArgs, scopeBackendResult, partitionBackendIds, matchesChatEnvelope } from "../src/backendInvoke.ts";
import { scopedId } from "../src/backendIdentity.ts";

const a = "http://localhost:7878", b = "https://other.example/loom";

test("same raw IDs route independently and nested mentions strip to wire IDs", async () => {
  const calls: unknown[] = [];
  const invoke = createBackendInvoke(async <T>(command: string, args: Record<string, unknown>) => { calls.push({ command, args }); return null as T; });
  for (const baseUrl of [a, b]) await invoke("send_input", {
    id: scopedId(baseUrl, "same"), mentions: [{ topicId: scopedId(baseUrl, "track"), resourceId: scopedId(baseUrl, "file:docs/a.md") }],
    configId: "model", model: "gpt-6", attachments: [{ name: "a.png", data: "123" }],
  });
  assert.deepEqual(calls, [a, b].map(baseUrl => ({ command: "send_input", args: {
    baseUrl, id: "same", mentions: [{ topicId: "track", resourceId: "file:docs/a.md" }],
    configId: "model", model: "gpt-6", attachments: [{ name: "a.png", data: "123" }],
  } })));
});

test("missing backend and mixed-server args never reach transport", async () => {
  let calls = 0;
  const invoke = createBackendInvoke(async <T>() => { calls++; return null as T; });
  await assert.rejects(invoke("launch_options"), /explicit/);
  await assert.rejects(invoke("send_to_thread", { sourceId: scopedId(a, "same"), destinationId: scopedId(b, "same") }), /different/);
  await assert.rejects(invoke("launch_session", { mentions: [{ resourceId: scopedId(b, "file") }] }, a), /different/);
  await assert.rejects(invoke("launch_options", { baseUrl: b }, a), /different/);
  assert.equal(calls, 0);
  assert.deepEqual(resolveBackendArgs({ repo: "org/repo" }, "HTTP://LOCALHOST:7878/"), { baseUrl: a, args: { repo: "org/repo", baseUrl: a } });
});

test("resources, todos, activity and action targets return scoped references; config IDs remain raw", () => {
  assert.deepEqual(scopeBackendResult("topic_resources", a, {
    resources: [{ id: "file:a.md" }], project: { id: "group" }, prs: [{ session_id: "same" }],
  }), { resources: [{ id: scopedId(a, "file:a.md") }], project: { id: scopedId(a, "group") }, prs: [{ session_id: scopedId(a, "same") }] });
  assert.deepEqual(scopeBackendResult("user_todos", a, { todos: [{ id: "todo", topic_id: "track" }] }), { todos: [{ id: scopedId(a, "todo"), topic_id: scopedId(a, "track") }] });
  const activity = scopeBackendResult("track_activity", a, { messages: [{ id: "message", channel_id: "channel", author_kind: "session", author_id: "same", payload: { child_session_id: "child" } }] });
  assert.equal(activity.messages[0].author_id, scopedId(a, "same"));
  assert.equal(activity.messages[0].payload.child_session_id, scopedId(a, "child"));
  assert.equal(scopeBackendResult("integrate_session", b, { coordinator_id: "same" }).coordinator_id, scopedId(b, "same"));
  const config = { config_options: [{ id: "model" }], agents: [{ models: [{ id: "gpt-6" }] }] };
  assert.deepEqual(scopeBackendResult("set_session_config", a, config), config);
  assert.deepEqual(scopeBackendResult("launch_options", a, config), config);
});

test("aggregate todo groups retain backend ownership despite colliding topic IDs", () => {
  assert.deepEqual([...partitionBackendIds([scopedId(a, "same"), scopedId(b, "same"), scopedId(a, "next")])], [
    [a, [scopedId(a, "same"), scopedId(a, "next")]], [b, [scopedId(b, "same")]],
  ]);
  assert.throws(() => partitionBackendIds(["raw"]), /qualified/);
  assert.deepEqual([...partitionBackendIds([])], []);
});

test("chat frames require both exact backend and session", () => {
  const session = scopedId(a, "same");
  assert.equal(matchesChatEnvelope(session, { server_url: `${a}/`, session_id: "same" }), true);
  assert.equal(matchesChatEnvelope(session, { server_url: b, session_id: "same" }), false);
  assert.equal(matchesChatEnvelope(session, { server_url: a, session_id: "other" }), false);
  assert.equal(matchesChatEnvelope(session, { server_url: "bad", session_id: "same" }), false);
});

test("freeform user text never selects a server or gets rewritten", async () => {
  const text = scopedId(b, "foreign-identity");
  const args = { id: scopedId(a, "session"), text, task: text, body: "loom:malformed", title: text, resource: { title: text } };
  const resolved = resolveBackendArgs(args);
  assert.equal(resolved.baseUrl, a);
  assert.deepEqual(resolved.args, { ...args, id: "session", baseUrl: a });
  assert.throws(() => resolveBackendArgs({ text }), /explicit/);
});

test("resource mention arrays and deleted-group destinations reach only their owner as raw IDs", async () => {
  const calls: any[] = [];
  const invoke = createBackendInvoke(async <T>(command: string, args: Record<string, unknown>) => { calls.push({ command, args }); return null as T; });
  await invoke("send_input", { id: scopedId(a, "session"), topicId: scopedId(a, "topic"), resourceIds: [scopedId(a, "file:a.md")] });
  await invoke("delete_group", { groupId: scopedId(b, "project"), destinationGroupId: scopedId(b, "inbox") });
  assert.deepEqual(calls, [
    { command: "send_input", args: { id: "session", topicId: "topic", resourceIds: ["file:a.md"], baseUrl: a } },
    { command: "delete_group", args: { groupId: "project", destinationGroupId: "inbox", baseUrl: b } },
  ]);
  await assert.rejects(invoke("delete_group", { groupId: scopedId(a, "project"), destinationGroupId: scopedId(b, "inbox") }), /different/);
});

test("embedded model and config IDs in activity payloads retain their wire meaning", () => {
  const reply = scopeBackendResult("track_activity", a, { messages: [{ id: "message", payload: {
    child_session_id: "child", model: { id: "model" }, config_options: [{ id: "effort" }],
  } }] });
  assert.equal(reply.messages[0].id, scopedId(a, "message"));
  assert.equal(reply.messages[0].payload.child_session_id, scopedId(a, "child"));
  assert.equal(reply.messages[0].payload.model.id, "model");
  assert.equal(reply.messages[0].payload.config_options[0].id, "effort");
});

test("hidden binding identity lists strip colon-rich keys and reject foreign owners before transport", async () => {
  const raw = "design_document:/repo:docs/design.md";
  const binding = scopeBackendResult("project_bindings", a, { bindings: [{ id: raw }], revision: 1 }).bindings[0];
  const calls: any[] = [];
  const invoke = createBackendInvoke(async <T>(command: string, args: any) => { calls.push({ command, args }); return null as T; });
  for (const key of ["hiddenBindingKeys", "hidden_binding_keys", "bindingIds", "binding_ids"]) {
    await invoke("launch_session", { project: { id: scopedId(a, "project") }, [key]: [binding.id, "file:/repo:a.md"] }, a);
    assert.deepEqual(calls.at(-1).args[key], [raw, "file:/repo:a.md"]);
    const count = calls.length;
    await assert.rejects(invoke("launch_session", { project: { id: scopedId(a, "project") }, [key]: [scopedId(b, raw)] }, a), /different/);
    assert.equal(calls.length, count);
  }
});

test("newer main policy, restore, Land and Publish commands keep owning backend identities", async () => {
  const wireSession = { id: "same", parent_id: null, parent_session_id: null, branch: { id: "branch", tags: [] } };
  for (const baseUrl of [a, b]) {
    const calls: { command: string; args: Record<string, unknown> }[] = [];
    const invoke = createBackendInvoke(async <T>(command: string, args: Record<string, unknown>) => {
      calls.push({ command, args });
      return (command === "set_autoland" ? { session: wireSession, warnings: ["saved"] }
        : command === "unarchive_session" ? wireSession : { coordinator_id: "same", target_branch: "arachne/tracks/accepted" }) as T;
    });
    const policy = await invoke<any>("set_autoland", { sessionId: scopedId(baseUrl, "same"), policy: "on" });
    assert.equal(policy.session.id, scopedId(baseUrl, "same"));
    assert.equal(policy.session.branch.id, scopedId(baseUrl, "branch"));
    assert.deepEqual(policy.warnings, ["saved"]);
    assert.equal((await invoke<any>("unarchive_session", { id: scopedId(baseUrl, "same") })).id, scopedId(baseUrl, "same"));
    for (const command of ["land_thread", "publish_track"]) {
      const result = await invoke<any>(command, { sessionId: scopedId(baseUrl, "same"), strategy: "squash" });
      assert.equal(result.coordinator_id, scopedId(baseUrl, "same"));
      assert.equal(result.target_branch, "arachne/tracks/accepted");
    }
    assert.ok(calls.every(call => call.args.baseUrl === baseUrl && (call.args.sessionId ?? call.args.id) === "same"));
  }
});
