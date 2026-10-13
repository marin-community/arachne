import assert from "node:assert/strict";
import test from "node:test";
import { nextTick } from "vue";
import { mountRoutingComponent } from "./componentRoutingHarness.ts";

const session = (id: string, parent: string | null = null, tags: any[] = []) => ({
  id, status: "running", parent_session_id: parent, parent_id: null,
  last_activity_at: "2026-10-11T00:00:00Z", placement: null,
  branch: { id: `branch-${id}`, branch: `weaver/${id}`, name: id, title: id, repo_root: "/repo", tags },
});
const idle = { key: "idle", value: "true" };
const contents = (el: any): string => (el.text ?? "") + el.children.map(contents).join("");

for (const name of ["FleetSidebar", "TopicInspector"]) {
  test(`${name} renders subtree activity and accessible attention without activity badges`, async t => {
    const root = session("track", null, [idle, { key: "topic", value: "true" }]);
    const middle = session("middle", "track", [idle]);
    const worker = session("worker", "middle", [{ key: "attention", value: "blocked", note: "Choose a release target" }]);
    const panel = mountRoutingComponent(name, { fleet: [root, middle, worker], topic: root,
      layout: null, selectedId: null, unarchivingIds: new Set() }, async () => [], true);
    t.after(panel.stop); await nextTick();
    if (name === "TopicInspector") { panel.state.tab = "threads"; await nextTick(); }
    const spinners = () => panel.elements(el => String(el.props.class).includes("spinner"));
    const badges = () => panel.elements(el => String(el.props.class).split(" ").includes("badge")).map(contents);
    assert.equal(spinners().length, name === "FleetSidebar" ? 1 : 3);
    if (name === "FleetSidebar") {
      panel.state.toggleTopicExpanded("track"); await nextTick();
      assert.equal(spinners().length, 3);
    }
    assert.equal(badges().some(label => /resting|working|reviewing|needs you/.test(label)), false);
    const attention = panel.elements(el => el.props.class === "thread-attention");
    assert.equal(attention.length, 1);
    assert.equal(contents(attention[0]), "!");
    assert.equal(attention[0].props.role, "img");
    assert.equal(attention[0].props["aria-label"], "Choose a release target");
    panel.props.fleet[2].branch.tags.push(idle); await nextTick();
    assert.equal(spinners().length, 0);
    assert.equal(panel.elements(el => el.props.class === "thread-attention").length, 1);
  });
}
