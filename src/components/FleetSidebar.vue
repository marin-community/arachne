<script setup lang="ts">
import { ref, computed } from "vue";
import type { SessionSummary, SessionLayout } from "../App.vue";

const props = defineProps<{
  fleet: SessionSummary[];
  layout: SessionLayout | null;
  selectedId: string | null;
  launching?: boolean;
}>();

const emit = defineEmits<{
  (e: "select", id: string): void;
  (e: "launch", task: string, repo: string, groupId?: string): void;
  (e: "create-workstream", name: string): void;
  (e: "move-session", sessionId: string): void;
}>();

const task = ref("");
const repo = ref("marin-community/arachne");
const newStreamName = ref("");
const showNewStream = ref(false);

// Which workstream a newly launched task should land in: the group currently
// in "launch focus", set when the user clicks inside a workstream's header.
const launchGroupId = ref<string | null>(null);

function submit() {
  const t = task.value.trim();
  if (!t) return;
  emit("launch", t, repo.value.trim(), launchGroupId.value ?? undefined);
  task.value = "";
}

function submitNewStream() {
  const n = newStreamName.value.trim();
  if (!n) return;
  emit("create-workstream", n);
  newStreamName.value = "";
  showNewStream.value = false;
}

// Loom tag semantics (weaver-core/src/tags.rs): the loud keys `attention`
// (agent self-report) and `triage` (outside assessment) carry values
// `attention` | `blocked`; absence is the calm/default state. `idle` is a
// quiet resting mark. Prose status lives on branch.description.
type Attention = { level: "attention" | "blocked"; source: string } | null;

function loudTag(s: SessionSummary): Attention {
  for (const key of ["attention", "triage"]) {
    const tag = s.branch.tags.find((t) => t.key === key);
    if (tag && (tag.value === "attention" || tag.value === "blocked")) {
      return { level: tag.value, source: tag.set_by };
    }
  }
  return null;
}

function subtitle(s: SessionSummary): string {
  return s.branch.description || s.branch.title || "—";
}

function statusClass(s: SessionSummary): string {
  if (s.status === "orphaned") return "orphaned";
  if (s.status === "running") {
    const loud = loudTag(s);
    if (loud?.level === "blocked") return "error";
    if (loud?.level === "attention") return "attention";
    return "running";
  }
  return "done";
}

function statusLabel(s: SessionSummary): string {
  if (s.status === "orphaned") return "orphan";
  const loud = loudTag(s);
  if (loud) return loud.level;
  if (s.status === "running") return "run";
  return s.status;
}

function isChild(s: SessionSummary): boolean {
  return !!s.parent_session_id || !!s.parent_id;
}

// --- Workstream grouping ---------------------------------------------------
//
// The layout view (spaces → groups) owns placement; summaries own status.
// A session appears in its placement group (falling back to "Inbox" when
// unplaced). Children nest under their parent session row; parents sort by
// last activity, newest first.

interface Row {
  session: SessionSummary;
  depth: number; // 0 = top-level, 1+ = nested child
}

interface Group {
  id: string;
  name: string;
  system: string | null;
  rows: Row[];
  activeCount: number;
}

function buildGroups(): Group[] {
  const byId = new Map(props.fleet.map((s) => [s.id, s]));

  // group_id -> session summaries placed there
  const inGroup = new Map<string, SessionSummary[]>();
  const unplaced: SessionSummary[] = [];
  for (const s of props.fleet) {
    if (s.status === "archived") continue;
    const gid = s.placement?.group_id;
    if (gid) {
      const list = inGroup.get(gid) ?? [];
      list.push(s);
      inGroup.set(gid, list);
    } else {
      unplaced.push(s);
    }
  }

  const groups: Group[] = [];
  const seen = new Set<string>();
  const addGroup = (
    id: string,
    name: string,
    system: string | null,
    members: SessionSummary[],
  ) => {
    if (seen.has(id)) return;
    seen.add(id);
    // Nest children under parents; order parents by activity desc.
    const parents = members
      .filter((s) => !isChild(s))
      .sort((a, b) =>
        a.last_activity_at < b.last_activity_at
          ? 1
          : a.last_activity_at === b.last_activity_at
            ? a.id < b.id
              ? 1
              : -1
            : -1,
      );
    const children = members.filter(isChild);
    const rows: Row[] = [];
    for (const p of parents) {
      rows.push({ session: p, depth: 0 });
      // children whose parent_session_id matches, else children by branch parent
      for (const c of children.filter(
        (c) => c.parent_session_id === p.id || c.parent_id === p.branch.id,
      )) {
        rows.push({ session: c, depth: 1 });
      }
    }
    // Orphaned children (parent archived or elsewhere) render flat.
    const claimed = new Set(rows.map((r) => r.session.id));
    for (const c of children.filter((c) => !claimed.has(c.id))) {
      rows.push({ session: c, depth: 0 });
    }
    groups.push({ id, name, system, rows, activeCount: rows.length });
  };

  if (props.layout) {
    for (const space of props.layout.spaces) {
      for (const g of space.groups) {
        const members = inGroup.get(g.id) ?? [];
        addGroup(g.id, g.name, g.system_key ?? null, members);
      }
    }
  }
  // Sessions whose placement group isn't in the layout (stale cache) or none.
  const placedElsewhere = props.fleet.filter(
    (s) =>
      s.status !== "archived" &&
      s.placement !== null &&
      s.placement.group_id !== null &&
      !seen.has(s.placement.group_id),
  );
  if (unplaced.length > 0 || placedElsewhere.length > 0) {
    addGroup("inbox", "Inbox", null, [...unplaced, ...placedElsewhere]);
  }
  return groups;
}

const groups = computed(() => buildGroups());
</script>

<template>
  <aside class="sidebar">
    <div class="new-task">
      <input
        v-model="task"
        placeholder="New task…"
        @keydown.enter.prevent="submit"
      />
      <button
        class="primary"
        :disabled="!task.trim() || props.launching"
        @click="submit"
      >
        {{ props.launching ? "…" : "Launch" }}
      </button>
    </div>
    <div class="new-task" style="margin-top: -4px">
      <input
        v-model="repo"
        placeholder="owner/name"
        spellcheck="false"
        style="font-family: var(--mono); font-size: 11px"
      />
    </div>
    <div v-if="launchGroupId" class="launch-target">
      launching into: {{ groups.find((g) => g.id === launchGroupId)?.name }}
      <button class="link" @click="launchGroupId = null">✕</button>
    </div>

    <div class="session-list">
      <template v-for="g in groups" :key="g.id">
        <div
          v-if="g.activeCount > 0 || !g.system"
          class="group-header"
          @click="launchGroupId = g.id === launchGroupId ? null : g.id"
        >
          <span class="group-name">{{ g.name }}</span>
          <span class="group-count">{{ g.activeCount }}</span>
          <button
            v-if="!g.system && g.id !== 'inbox'"
            class="link stream-delete"
            title="delete workstream (sessions move to Inbox)"
            @click.stop="emit('move-session', `__delete__:${g.id}`)"
          >
            ✕
          </button>
        </div>
        <div
          v-for="row in g.rows"
          :key="row.session.id"
          class="session-item"
          :class="{
            selected: row.session.id === selectedId,
            child: row.depth > 0,
          }"
          :style="
            row.depth > 0 ? { paddingLeft: `${12 + row.depth * 14}px` } : {}
          "
          @click="emit('select', row.session.id)"
        >
          <div class="row1">
            <span class="name">{{
              row.session.branch.name || row.session.id
            }}</span>
            <span class="badge" :class="statusClass(row.session)">{{
              statusLabel(row.session)
            }}</span>
            <span
              v-if="row.session.branch.tags.some((t) => t.key === 'idle')"
              class="badge idle"
              >idle</span
            >
          </div>
          <div class="title">{{ subtitle(row.session) }}</div>
        </div>
      </template>
      <div
        v-if="groups.length === 0"
        class="session-item"
        style="color: var(--text-dim)"
      >
        No sessions yet — launch one above.
      </div>
    </div>

    <div class="new-stream" v-if="showNewStream">
      <input
        v-model="newStreamName"
        placeholder="workstream name…"
        @keydown.enter.prevent="submitNewStream"
        @keydown.esc="showNewStream = false"
      />
    </div>
    <button v-else class="link new-stream-btn" @click="showNewStream = true">
      + workstream
    </button>
  </aside>
</template>
