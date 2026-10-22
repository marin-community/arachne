import assert from "node:assert/strict";
import test from "node:test";
import { nextTick } from "vue";
import { scopeSession, scopedId } from "../src/backendIdentity.ts";
import { pinSelection } from "../src/durableChange.ts";
import { declaration } from "./fixtures/durable-change.mjs";
import { deferred, mountRoutingComponent, settle } from "./componentRoutingHarness.ts";

const a = "https://a.example", b = "https://b.example";
const bindingKey = "design_document:/repo:docs/design.md";
const topic = (server: string, id = "track") => scopeSession(server, {
  id, status: "idle", parent_id: null, parent_session_id: null, placement: null,
  worktree_present: false, work_dir: "", github_repo: "org/own", last_activity_at: "2026-10-10T10:00:00Z",
  branch: { id: `branch-${id}`, name: id, title: id, repo_root: "/repo", branch: `weaver/${id}`, tags: [{ key: "topic", value: "true" }] },
} as any);
const resources = { resources: [], hidden_resources: [], revision: 0, issues: [], prs: [] };
// Visible text of a harness element (its text plus all descendants).
function textOf(el: any): string {
  if (el.type === "text") return String(el.text ?? "");
  return [String(el.text ?? ""), ...el.children.map(textOf)].join("");
}

test("Home completion controls disable only the pending Todo's owning row", async t => {
  const pendingB = deferred<void>();
  let aToggles = 0;
  const home = mountRoutingComponent("HomeView", { fleet: [topic(a), topic(b)], topic: null, project: null, layout: null }, async (command, args) => {
    if (command === "toggle_todo") {
      if (args.baseUrl === b) return pendingB.promise;
      aToggles++; return;
    }
    return { todos: [{ id: "same-todo", topic_id: "track", text: "Work", revision: 1 }], warnings: [] };
  }, true);
  t.after(home.stop); await settle();
  const rowB = home.state.visibleTodos.find((row: any) => row.topic_id === scopedId(b, "track"));
  const completionB = home.state.completeTodo(rowB); await nextTick();
  const controls = home.elements(el => el.props.class === "home-todo-complete");
  assert.equal(controls.length, 2);
  assert.equal(Boolean(controls[0].props.disabled), false, "healthy A remains actionable");
  assert.equal(Boolean(controls[1].props.disabled), true, "pending B is locked");
  await controls[0].props.onClick(); assert.equal(aToggles, 1);
  pendingB.resolve(); await completionB;
});

test("ProjectSheet can switch away from an unavailable backend and dismiss while mutations remain disabled", async t => {
  const calls: string[] = [];
  const sheet = mountRoutingComponent("ProjectSheet", { backendUrl: a, backends: [{ url: a, name: "A", available: false }, { url: b, name: "B", available: true }], available: false }, async command => { calls.push(command); return []; }, true);
  t.after(sheet.stop); await settle();
  sheet.state.name = "Offline draft"; sheet.state.repo = "/repo"; await nextTick();
  const picker = sheet.elements(el => el.type === "select")[0];
  assert.equal(Boolean(picker.props.disabled), false, "offline A must not disable the backend picker");
  picker.props.onChange({ target: { value: b } });
  assert.deepEqual(sheet.events.find(event => event.name === "backend")?.args, [b]);
  const close = sheet.elements(el => el.props["aria-label"] === "Close project settings")[0];
  const cancel = sheet.elements(el => el.type === "button" && el.text === "Cancel")[0];
  for (const control of [close, cancel]) { assert.equal(Boolean(control.props.disabled), false); control.props.onClick(); }
  assert.equal(sheet.events.filter(event => event.name === "close").length, 2);
  const create = sheet.elements(el => el.type === "button" && el.text?.trim() === "Create project")[0];
  assert.equal(Boolean(create.props.disabled), true);
  await sheet.state.create();
  await sheet.state.onAttachDraft({ kind: "file", title: "Design", repository: "", reference: null, path: "docs/design.md", url: null });
  await sheet.state.remove({ id: scopedId(a, bindingKey) });
  assert.equal(calls.filter(command => ["create_group", "add_project_binding", "remove_project_binding"].includes(command)).length, 0);
  sheet.props.backendUrl = b; sheet.props.available = true; await nextTick();
  assert.equal(Boolean(sheet.elements(el => el.type === "select")[0].props.disabled), false);
  assert.equal(Boolean(sheet.elements(el => el.type === "button" && el.text?.trim() === "Create project")[0].props.disabled), false);
});

test("unchecked inherited bindings launch as raw keys through the actual NewTrackChat and bridge", async t => {
  const calls: { command: string; args: any }[] = [];
  const chat = mountRoutingComponent("NewTrackChat", { backendUrl: a, backends: [], available: true, fleet: [], launchOptions: null, project: { id: scopedId(a, "project"), name: "Project" } }, async (command, args) => {
    calls.push({ command, args });
    if (command === "project_bindings") return { bindings: [{ id: bindingKey, title: "Design", kind: "design_document", repository: "/repo", path: "docs/design.md" }], revision: 1 };
    if (command === "launch_session") return { ...topic(a), id: "new-track", branch: { id: "new-branch", tags: [] } };
    return [];
  });
  t.after(chat.stop);
  chat.state.repo = "/repo"; await nextTick(); await settle();
  assert.equal(chat.state.projectBindings[0].id, scopedId(a, bindingKey));
  chat.state.toggleBinding(chat.state.projectBindings[0].id, false);
  chat.state.body = "Launch without the inherited design"; chat.state.send();
  const submittedDraft = localStorage.getItem(`arachne.newTopicDraft@${a}`);
  assert.equal(JSON.parse(submittedDraft!).body, "Launch without the inherited design", "actual composer saves the submitted snapshot before launch delivery");
  const launch = chat.events.find(event => event.name === "launch")!;
  assert.deepEqual(launch.args[2].hiddenBindingKeys, [bindingKey]);
  // The parent uses the same bridge for launch and persists these raw keys.
  const { createBackendInvoke } = await import("../src/backendInvoke.ts");
  const invoke = createBackendInvoke(async <T>(command: string, args: any) => { calls.push({ command, args }); return { id: "new", branch: { id: "branch", tags: [] }, placement: null } as T; });
  await invoke("launch_session", { task: launch.args[0], project: launch.args[2].project, hiddenBindingKeys: launch.args[2].hiddenBindingKeys }, a);
  assert.deepEqual(calls.at(-1)?.args.hiddenBindingKeys, [bindingKey]);
  chat.state.toggleBinding(scopedId(b, bindingKey), false); chat.state.send();
  assert.equal(chat.events.filter(event => event.name === "launch").length, 1, "foreign hidden binding cannot bypass owner validation");
  assert.match(chat.state.projectBindingsError, /different/);
  assert.equal(localStorage.getItem(`arachne.newTopicDraft@${a}`), submittedDraft, "rejected launch cannot replace the submitted snapshot");
});

test("a pending backend catalog remains usable after switching tracks on that backend", async t => {
  const catalog = deferred<{ slug: string }[]>(); let requests = 0;
  const panel = mountRoutingComponent("ResourcePanel", { topic: topic(a, "one") }, async command => {
    if (command === "topic_resources") return resources;
    if (command === "managed_repos") { requests++; return catalog.promise; }
    return null;
  });
  t.after(panel.stop); await settle();
  panel.props.topic = topic(a, "two"); await nextTick();
  catalog.resolve([{ slug: "org/catalog" }]); await settle();
  assert.equal(requests, 1, "the catalog is owned by the backend, so the pending request stays valid");
  assert.deepEqual(panel.state.managedRepos, ["org/catalog"]);
  assert.ok(panel.state.repoOptions.includes("org/catalog"));
});

test("an old A catalog failure cannot clear the successfully loaded B catalog", async t => {
  const old = deferred<unknown>();
  const panel = mountRoutingComponent("ResourcePanel", { topic: topic(a) }, async (command, args) => {
    if (command === "topic_resources") return resources;
    if (command === "managed_repos") return args.baseUrl === a ? old.promise : [{ slug: "org/b-only" }];
    return null;
  });
  t.after(panel.stop); await settle();
  panel.props.topic = topic(b); await nextTick(); await settle();
  assert.deepEqual(panel.state.managedRepos, ["org/b-only"]);
  old.reject(new Error("A unavailable")); await settle();
  assert.deepEqual(panel.state.managedRepos, ["org/b-only"]);
});

test("actual ChangeReview preparation sends a raw coordinator identity while preserving user text", async t => {
  const calls: { command: string; args: any }[] = [];
  const selected = declaration();
  const review = mountRoutingComponent("ChangeReview", { backendUrl: a, sessionId: scopedId(a, "worker"), selectedChange: pinSelection(scopedId(a, "worker"), selected), integrationTarget: topic(a, "coordinator"), isTopic: false }, async (command, args) => {
    calls.push({ command, args });
    if (command === "change_changes") return { source_ref: "weaver/worker", accepted_source: false, head_oid: selected.scope.head_oid, base: { state: "available", reference: selected.scope.base_oid, oid: selected.scope.base_oid }, files: [], totals: {} };
    return null;
  });
  t.after(review.stop); await settle();
  review.state.prepareRequest("prepare");
  assert.match(review.state.requestText, /session coordinator\)/);
  assert.ok(!review.state.requestText.includes(scopedId(a, "coordinator")));
  const userText = `User note: keep ${scopedId(b, "literal-user-text")} unchanged.`;
  review.state.requestText += ` ${userText}`;
  await review.state.sendRequest();
  const sent = calls.find(call => call.command === "send_input")!;
  assert.equal(sent.args.baseUrl, a); assert.equal(sent.args.id, "worker");
  assert.match(sent.args.text, /session coordinator\)/);
  assert.ok(sent.args.text.includes(userText));
  assert.ok(sent.args.text.includes(JSON.stringify(selected)), "selected declaration remains mandatory after user edits");
});

test("actual ChangeReview inline comments send through the plain send_input path", async t => {
  const calls: { command: string; args: any }[] = [];
  const line = (kind: string, old_line: number | null, new_line: number | null, text: string) => ({ kind, old_line, new_line, text });
  const review = mountRoutingComponent("ChangeReview", { backendUrl: a, sessionId: scopedId(a, "worker"), selectedChange: null, integrationTarget: null, isTopic: false }, async (command, args) => {
    calls.push({ command, args });
    if (command === "work_changes") return {
      source_ref: "weaver/worker", accepted_source: false, version: null, head_oid: "e".repeat(40),
      base: { state: "available", reference: "main", oid: "a".repeat(40) },
      totals: { files: 1, additions: 1, deletions: 1, truncated: false }, truncated: false,
      files: [{
        status: "modified", path: { display: "src/app.ts", bytes: "src/app.ts" }, old_path: null, sources: [],
        additions: 1, deletions: 1, content: "text", truncated: false,
        hunks: [{ header: "@@ -1,3 +1,3 @@", truncated: false, lines: [
          line("context", 1, 1, "const a = 1;"),
          line("deletion", 2, null, "const b = 2;"),
          line("addition", null, 2, "const b = 3;"),
        ] }],
      }],
    };
    return null;
  }, true);
  t.after(review.stop); await settle();
  assert.equal(review.elements(el => el.props.class?.startsWith("change-review-line")).length, 3, "rendered diff lines expose comment affordances");
  const buttons = review.elements(el => el.props["aria-label"]?.startsWith("Add an inline comment on "));
  assert.equal(buttons.length, 3);
  assert.equal(buttons[0].props["aria-label"], "Add an inline comment on src/app.ts:1");
  // Deletions fall back to the old-side line number in their anchor.
  const deleteButton = review.elements(el => el.props["aria-label"] === "Add an inline comment on src/app.ts:2 (deleted line)")[0];
  await deleteButton.props.onClick({ stopPropagation() {} }); await nextTick();
  assert.ok(review.state.activeComment, "clicking the + button opens one comment box");
  review.state.commentDraft = "Do we need this flag at all?"; await nextTick();
  const commentButtons = review.elements(el => el.type === "button" && el.text === "Comment");
  await commentButtons[0].props.onClick(); await nextTick();
  assert.equal(review.state.comments.length, 1);
  assert.equal(review.state.comments[0].anchor, "src/app.ts:2");
  assert.equal(review.state.comments[0].text, "Do we need this flag at all?");
  assert.equal(review.state.activeComment, null, "the box closes after adding");
  const cards = review.elements(el => el.props.class === "change-review-comment-card");
  assert.equal(cards.length, 1, "the added comment renders as a pinned card");
  // Queue a second comment on an addition line.
  await buttons[2].props.onClick({ stopPropagation() {} }); await nextTick();
  review.state.commentDraft = "Something looks off here."; await nextTick();
  await review.elements(el => el.type === "button" && el.text === "Comment")[0].props.onClick(); await nextTick();
  assert.equal(review.state.comments.length, 2);
  // Send to agent: the composer area opens, then Send invokes plain send_input.
  review.state.openCommentSend(); await nextTick();
  const chip = review.elements(el => el.props.class === "change-review-comments-chip")[0];
  assert.equal(chip.text, "2 comments");
  review.state.commentMessage = "Please take a look."; await nextTick();
  await review.state.sendComments(); await settle();
  const sent = calls.filter(call => call.command === "send_input");
  assert.equal(sent.length, 1);
  assert.equal(sent[0].args.baseUrl, a); assert.equal(sent[0].args.id, "worker");
  assert.equal(sent[0].args.topicId, null); assert.deepEqual(sent[0].args.resourceIds, []); assert.deepEqual(sent[0].args.attachments, []);
  assert.equal(sent[0].args.changeId, undefined, "inline comments never use the selected-preparation path");
  assert.ok(sent[0].args.text.includes("2 inline comments on the current review"));
  assert.ok(sent[0].args.text.includes("- `src/app.ts:2` — Do we need this flag at all?"));
  assert.ok(sent[0].args.text.includes("- `src/app.ts:2` — Something looks off here."));
  assert.ok(sent[0].args.text.includes("Please take a look."));
  assert.equal(review.state.comments.length, 0, "comments clear after sending");
  assert.match(review.state.requestNote, /Sent 2 inline comments to this thread\./);
  assert.equal(review.events.filter(event => event.name === "requested").length, 1);
  // A deletion at old line 2 and an addition at new line 2 share the anchor
  // path:2 but stay pinned to their own rows.
  const additionCards = review.elements(el => el.props.class === "change-review-comment-card");
  assert.equal(additionCards.length, 0, "sending cleared the pinned cards");
});

test("actual ChangeReview range comments cover clicked line ranges and send through send_input", async t => {
  const calls: { command: string; args: any }[] = [];
  const line = (kind: string, old_line: number | null, new_line: number | null, text: string) => ({ kind, old_line, new_line, text });
  const review = mountRoutingComponent("ChangeReview", { backendUrl: a, sessionId: scopedId(a, "worker"), selectedChange: null, integrationTarget: null, isTopic: false }, async (command, args) => {
    calls.push({ command, args });
    if (command === "work_changes") return {
      source_ref: "weaver/worker", accepted_source: false, version: null, head_oid: "e".repeat(40),
      base: { state: "available", reference: "main", oid: "a".repeat(40) },
      totals: { files: 1, additions: 3, deletions: 1, truncated: false }, truncated: false,
      files: [{
        status: "modified", path: { display: "src/foo.ts", bytes: "src/foo.ts" }, old_path: null, sources: [],
        additions: 3, deletions: 1, content: "text", truncated: false,
        hunks: [{ header: "@@ -1,4 +1,6 @@", truncated: false, lines: [
          line("context", 1, 1, "const a = 1;"),
          line("deletion", 2, null, "const b = 2;"),
          line("addition", null, 2, "const b = 3;"),
          line("addition", null, 3, "const c = 4;"),
          line("context", 3, 4, "const d = 5;"),
          line("addition", null, 5, "const e = 6;"),
        ] }],
      }],
    };
    return null;
  }, true);
  t.after(review.stop); await settle();
  const rows = review.elements(el => el.props.class?.startsWith("change-review-line"));
  assert.equal(rows.length, 6, "the rendered diff exposes selectable rows");
  // A plain click selects one line as the anchor without opening a box.
  await rows[0].props.onClick({ shiftKey: false }); await nextTick();
  assert.equal(review.state.activeComment, null, "clicking a line never opens the single-line box directly");
  assert.equal(review.state.activeRangeComment, null);
  assert.equal(review.state.selectionAnchorLabel, "src/foo.ts:1");
  assert.equal(review.state.selectionCount, 1);
  assert.equal(review.elements(el => el.props.class === "change-review-range-affordance").length, 1, "a one-line selection shows the range affordance");
  // Shift-click extends the selection to the full range between the rows.
  await rows[4].props.onClick({ shiftKey: true }); await nextTick();
  assert.equal(review.state.selectionCount, 5, "shift-click covers every row between the endpoints");
  assert.equal(review.state.selectionAnchorLabel, "src/foo.ts:1-4");
  const selected = review.elements(el => el.props.class?.includes("change-review-line-selected"));
  assert.equal(selected.length, 5, "each covered row highlights");
  // A click outside the range clears the selection again.
  await rows[5].props.onClick({ shiftKey: false }); await nextTick();
  assert.equal(review.state.selectionCount, 1, "a plain click outside the range restarts the anchor");
  await rows[4].props.onClick({ shiftKey: true }); await nextTick();
  assert.equal(review.state.selectionAnchorLabel, "src/foo.ts:4-5");
  // Escape clears the selection.
  review.state.onSelectionKeydown({ key: "Escape" }); await nextTick();
  assert.equal(review.state.selectionCount, 0);
  assert.equal(review.elements(el => el.props.class === "change-review-range-affordance").length, 0);
  // Re-select and open the range comment box via the range affordance.
  await rows[0].props.onClick({ shiftKey: false }); await nextTick();
  await rows[4].props.onClick({ shiftKey: true }); await nextTick();
  const affordance = review.elements(el => el.props.class === "change-review-range-button")[0];
  assert.match(affordance.props["aria-label"], /Add an inline comment on src\/foo\.ts:1-4 \(5 selected lines\)/);
  assert.equal(affordance.text, "Comment on 5 lines");
  await affordance.props.onClick(); await nextTick();
  assert.ok(review.state.activeRangeComment, "the range affordance opens the range composer");
  assert.equal(review.state.activeComment, null);
  const box = review.elements(el => el.props.class === "change-review-comment-box")[0];
  assert.match(box.props["aria-label"], /Range comment on src\/foo\.ts:1-4/);
  const label = review.elements(el => el.type === "label")[0];
  assert.ok(textOf(label).includes("src/foo.ts:1-4"), "the composer states the range");
  // Adding the comment pins a range card at the first line of the range.
  review.state.commentDraft = "Extract this block."; await nextTick();
  await review.elements(el => el.type === "button" && el.text === "Comment")[0].props.onClick(); await nextTick();
  assert.equal(review.state.comments.length, 1);
  const range = review.state.comments[0];
  assert.equal(range.anchor, "src/foo.ts:1-4");
  assert.deepEqual(range.range, { first: 1, last: 4 });
  assert.equal(range.line, 1);
  assert.equal(range.text, "Extract this block.");
  const cards = review.elements(el => el.props.class === "change-review-comment-card change-review-comment-range"
    || el.props.class === "change-review-comment-card");
  assert.equal(cards.length, 1, "the added range comment renders a card");
  const kinds = review.elements(el => el.props.class?.includes("change-review-comment-range-kind"));
  assert.equal(kinds.length, 1, "the card shows the covered range");
  assert.equal(textOf(kinds[0]), "1-4");
  assert.equal(review.state.activeRangeComment, null, "the range composer closes after adding");
  assert.equal(review.state.selectionCount, 0, "adding clears the selection");
  // Sending includes the range anchor in the emitted prompt.
  review.state.openCommentSend(); await nextTick();
  review.state.commentMessage = "Please take a look."; await nextTick();
  await review.state.sendComments(); await settle();
  const sent = calls.filter(call => call.command === "send_input");
  assert.equal(sent.length, 1);
  assert.ok(sent[0].args.text.includes("1 inline comment on the current review"));
  assert.ok(sent[0].args.text.includes("- `src/foo.ts:1-4` — Extract this block."));
  assert.ok(sent[0].args.text.includes("Please take a look."));
  assert.equal(review.state.comments.length, 0, "range comments clear after sending");
  assert.match(review.state.requestNote, /Sent 1 inline comment to this thread\./);
});

test("an older catalog request cannot erase a later generation after returning to A", async t => {
  const oldA = deferred<unknown>(); let aRequests = 0;
  const panel = mountRoutingComponent("ResourcePanel", { topic: topic(a) }, async (command, args) => {
    if (command === "topic_resources") return resources;
    if (command === "managed_repos") {
      if (args.baseUrl === a) return ++aRequests === 1 ? oldA.promise : [{ slug: "org/new-a" }];
      return [{ slug: "org/b" }];
    }
    return null;
  });
  t.after(panel.stop); await settle();
  panel.props.topic = topic(b); await nextTick(); await settle();
  panel.props.topic = topic(a); await nextTick(); await settle();
  assert.deepEqual(panel.state.managedRepos, ["org/new-a"]);
  oldA.reject(new Error("retired A request")); await settle();
  assert.deepEqual(panel.state.managedRepos, ["org/new-a"]);
});
