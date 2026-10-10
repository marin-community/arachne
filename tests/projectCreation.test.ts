import assert from "node:assert/strict";
import test from "node:test";
import { saveNewProject, type ProjectBinding, type ProjectCreationDraft } from "../src/projectCreation.ts";

const group = (id: string, name = "Project") => ({ id, name, rank: 1, system_key: null, collapsed: false, session_ids: [] });
const layout = {
  revision: 1,
  spaces: [{ id: "user", name: "User", rank: 0, system_key: "user", groups: [group("existing")] }],
  defaults: [],
};
const createdLayout = { ...layout, revision: 2, spaces: [{ ...layout.spaces[0], groups: [...layout.spaces[0].groups, group("new")] }] };
const resource = (id: string): ProjectBinding => ({ id, kind: "file", title: id, repository: "", reference: null, path: `${id}.md`, url: null });
const options = { name: " Project ", repo: " org/repo ", layout };
const noop = () => {};

test("creating a project without resources only creates its group", async () => {
  const draft: ProjectCreationDraft = { project: null, bindings: [] };
  const layouts: unknown[] = [];
  const commands: string[] = [];
  const project = await saveNewProject(draft, { ...options, repo: "" }, async (command, args) => {
    commands.push(command);
    assert.deepEqual(args, { spaceId: "user", name: "Project" });
    return createdLayout as any;
  }, (value) => layouts.push(value), noop);
  assert.deepEqual(project, { id: "new", name: "Project" });
  assert.deepEqual(commands, ["create_group"]);
  assert.deepEqual(layouts, [createdLayout]);
});

test("queued resources use the returned project id and successive store revisions", async () => {
  const drafts = [resource("design"), { ...resource("repo"), kind: "repository", repository: "org/other", path: null },
    { ...resource("pr"), kind: "pull_request", path: null, url: "https://github.com/org/repo/pull/42" }];
  const draft: ProjectCreationDraft = { project: null, bindings: [...drafts] };
  const saved: ProjectBinding[] = [];
  let revision = 12;
  let reads = 0;
  await saveNewProject(draft, options, async (command, args) => {
    if (command === "create_group") return createdLayout as any;
    assert.equal(args.projectId, "new");
    assert.equal(args.repo, "org/repo");
    if (command === "project_bindings") {
      reads++;
      return { bindings: [], revision } as any;
    }
    assert.equal(command, "add_project_binding");
    assert.equal(args.expectedRevision, revision);
    const { id, ...expected } = drafts[saved.length];
    assert.deepEqual(args.resource, expected);
    saved.push({ id: `saved-${id}`, ...expected });
    return { bindings: [...saved], revision: ++revision } as any;
  }, noop, noop);
  assert.equal(reads, 1);
  assert.equal(saved.length, 3);
  assert.deepEqual(draft.bindings, []);
});

test("retry after a partial resource failure reuses the project and saves only pending resources", async () => {
  const draft: ProjectCreationDraft = { project: null, bindings: [resource("first"), resource("second")] };
  let creates = 0;
  let fail = true;
  let revision = 5;
  const saved: ProjectBinding[] = [];
  const invoke = async <T>(command: string, args: Record<string, unknown>): Promise<T> => {
    if (command === "create_group") { creates++; return createdLayout as T; }
    if (command === "add_project_binding") {
      assert.equal(args.expectedRevision, revision);
      const binding = args.resource as ProjectBinding;
      if (binding.title === "second" && fail) throw new Error("store conflict");
      saved.push({ ...binding, id: `saved-${binding.title}` });
      revision++;
    }
    return { bindings: [...saved], revision } as T;
  };
  await assert.rejects(saveNewProject(draft, options, invoke, noop, noop), /store conflict/);
  assert.deepEqual(draft.project, { id: "new", name: "Project" });
  assert.deepEqual(draft.bindings.map((b) => b.title), ["second"]);
  assert.equal(draft.savedRepo, "org/repo");
  await assert.rejects(saveNewProject(draft, { ...options, repo: "org/other" }, invoke, noop, noop), /original repository/);
  assert.deepEqual(saved.map((b) => b.title), ["first"]);
  fail = false;
  revision++; // A concurrent edit: the retry must read the current revision.
  await saveNewProject(draft, { ...options, layout: createdLayout }, invoke, noop, noop);
  assert.equal(creates, 1);
  assert.deepEqual(saved.map((b) => b.title), ["first", "second"]);
  assert.deepEqual(draft.bindings, []);
});

test("a bindings read failure leaves the created project and queued resources available for retry", async () => {
  const draft: ProjectCreationDraft = { project: null, bindings: [resource("file")] };
  await assert.rejects(saveNewProject(draft, options, async (command) => {
    if (command === "create_group") return createdLayout as any;
    throw new Error("repository unavailable");
  }, noop, noop), /repository unavailable/);
  assert.equal(draft.project?.id, "new");
  assert.equal(draft.bindings.length, 1);
  assert.equal(draft.savedRepo, undefined);
});

test("a failed first resource write allows correcting the repository without creating another group", async () => {
  const draft: ProjectCreationDraft = { project: null, bindings: [resource("file")] };
  await assert.rejects(saveNewProject(draft, options, async (command) => {
    if (command === "create_group") return createdLayout as any;
    if (command === "project_bindings") return { bindings: [], revision: 1 } as any;
    throw new Error("repository unavailable");
  }, noop, noop), /repository unavailable/);
  assert.equal(draft.savedRepo, undefined);
  await saveNewProject(draft, { ...options, repo: "org/corrected" }, async (command, args) => {
    assert.notEqual(command, "create_group");
    assert.equal(args.repo, "org/corrected");
    return { bindings: [], revision: command === "project_bindings" ? 1 : 2 } as any;
  }, noop, noop);
  assert.equal(draft.savedRepo, "org/corrected");
  assert.deepEqual(draft.bindings, []);
});

test("validation failures do not create projects or consume queued resources", async () => {
  const draft: ProjectCreationDraft = { project: null, bindings: [resource("file")] };
  const unexpectedInvoke = async <T>(): Promise<T> => { throw new Error("unexpected invoke"); };
  for (const [overrides, message] of [
    [{ name: " " }, /project name/],
    [{ repo: " " }, /repository/],
    [{ layout: null }, /No space/],
  ] as const) {
    await assert.rejects(saveNewProject(draft, { ...options, ...overrides }, unexpectedInvoke, noop, noop), message);
  }
  assert.equal(draft.project, null);
  assert.equal(draft.bindings.length, 1);
});

test("a group creation failure preserves the draft", async () => {
  const draft: ProjectCreationDraft = { project: null, bindings: [resource("file")] };
  await assert.rejects(saveNewProject(draft, options, async () => { throw new Error("offline"); }, noop, noop), /offline/);
  assert.equal(draft.project, null);
  assert.equal(draft.bindings.length, 1);
});

test("leaving setup during creation stops further writes and layout updates", async () => {
  const draft: ProjectCreationDraft = { project: null, bindings: [resource("file")] };
  let active = true;
  const commands: string[] = [];
  await assert.rejects(saveNewProject(draft, { ...options, isActive: () => active }, async (command) => {
    commands.push(command);
    active = false;
    return createdLayout as any;
  }, () => assert.fail("closed setup must not update the layout"), noop), /setup was closed/);
  assert.deepEqual(commands, ["create_group"]);
  assert.equal(draft.project?.id, "new");
  assert.equal(draft.bindings.length, 1);
});
