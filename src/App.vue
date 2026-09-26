<script setup lang="ts">
import { ref, onMounted, computed } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import FleetSidebar from "./components/FleetSidebar.vue";
import ThreadView from "./components/ThreadView.vue";
import SettingsSheet from "./components/SettingsSheet.vue";

// --- Types mirroring src-tauri/src/loom.rs (snake_case wire) ---------------

interface TagView {
  key: string;
  note: string;
  value: string;
  set_at: string;
  set_by: string;
}
interface BranchSummary {
  id: string;
  branch: string;
  name: string;
  title: string;
  description: string;
  goal: string;
  repo_root: string;
  tags: TagView[];
}
interface Placement {
  space_id: string | null;
  space_name: string | null;
  group_id: string | null;
  group_name: string | null;
  group_system_key: string | null;
  rank: number | null;
  session_id: string | null;
}
export interface SessionSummary {
  id: string;
  status: string;
  profile: string;
  class: string;
  origin: string;
  created_by: string | null;
  created_at: string;
  last_activity_at: string;
  branch: BranchSummary;
  placement: Placement | null;
  github_repo: string | null;
  parent_id: string | null;
  parent_session_id: string | null;
}
export interface SessionView {
  id: string;
  status: string;
  profile: string;
  class: string;
  origin: string;
  agent_kind: string;
  model: string;
  effort: string;
  protocol: string;
  work_dir: string;
  term_session: string;
  turn_count: number;
  created_by: string | null;
  created_at: string;
  last_activity_at: string;
  branch: BranchSummary;
  placement: Placement | null;
}
export interface SessionGroup {
  id: string;
  space_id: string;
  name: string;
  rank: number;
  system_key: string | null;
  collapsed: boolean;
  session_ids: string[];
}
export interface SessionSpace {
  id: string;
  name: string;
  rank: number;
  system_key: string | null;
  groups: SessionGroup[];
}
export interface SessionLayout {
  revision: number;
  spaces: SessionSpace[];
  defaults: {
    selector_kind: string;
    selector_value: string;
    group_id: string;
  }[];
}
interface FleetSnapshot {
  sessions: SessionSummary[];
  layout: SessionLayout;
}

// --- State ----------------------------------------------------------------

const DEFAULT_URL = "http://127.0.0.1:7878";
const connected = ref(false);
const connError = ref<string | null>(null);
const launching = ref(false);
const fleet = ref<SessionSummary[]>([]);
const layout = ref<SessionLayout | null>(null);
const selectedId = ref<string | null>(null);
const selectedView = ref<SessionView | null>(null);
const showSettings = ref(false);
const loomUrl = ref(localStorage.getItem("loomUrl") ?? DEFAULT_URL);
const loomToken = ref(localStorage.getItem("loomToken") ?? "");

// --- Boot: connect to loom (URL from settings) ------------------------------

onMounted(async () => {
  // Register listeners BEFORE connecting, so the initial fleet snapshot
  // emitted right after connect is never missed.
  await listen<FleetSnapshot>("loom://fleet", (event) => {
    fleet.value = event.payload.sessions;
    layout.value = event.payload.layout;
  });
  await listen("loom://launched", (event) => {
    const view = event.payload as SessionView;
    selectedId.value = view.id;
    selectedView.value = view;
  });
  await listen("loom://error", (event) => {
    const err = event.payload as { message: string; unreachable: boolean };
    if (err.unreachable) {
      connected.value = false;
      connError.value = err.message;
    }
  });

  await connect(loomUrl.value, loomToken.value || null);
});

async function connect(url: string, token: string | null) {
  try {
    await invoke("connect", { baseUrl: url, token });
    connected.value = true;
    connError.value = null;
  } catch (e: any) {
    connected.value = false;
    connError.value = e?.message ?? String(e);
  }
}

function saveSettings(url: string, token: string) {
  loomUrl.value = url;
  loomToken.value = token;
  localStorage.setItem("loomUrl", url);
  localStorage.setItem("loomToken", token);
  showSettings.value = false;
  connect(url, token || null);
}

// --- Actions ----------------------------------------------------------------

async function selectSession(id: string) {
  selectedId.value = id;
  // Drop the stale view immediately: ThreadView is keyed by selectedId and
  // remounts the moment it changes — if the old view were still here, its
  // onMounted would fetch the PREVIOUS session's chat, and nothing would
  // re-run when the fresh view arrives (clicked row N, saw row N±1's thread).
  selectedView.value = null;
  try {
    selectedView.value = await invoke<SessionView>("open_session", { id });
  } catch (e: any) {
    connError.value = e?.message ?? String(e);
  }
}

async function launchTask(task: string, repo: string) {
  launching.value = true;
  try {
    const view = await invoke<SessionView>("launch_session", { repo, task });
    selectedId.value = view.id;
    selectedView.value = view;
  } catch (e: any) {
    connError.value = e?.message ?? String(e);
  } finally {
    launching.value = false;
  }
}

async function delegateFromThread(parentId: string, task: string) {
  launching.value = true;
  try {
    const view = await invoke<SessionView>("delegate_task", { parentId, task });
    // Stay on the parent thread — the child appears nested under it in the
    // sidebar (and inherits the parent's workstream) via the layout events.
  } catch (e: any) {
    connError.value = e?.message ?? String(e);
  } finally {
    launching.value = false;
  }
}

async function createWorkstream(name: string) {
  try {
    await invoke("create_workstream", { name });
  } catch (e: any) {
    connError.value = e?.message ?? String(e);
  }
}

async function reparentSession(
  sessionId: string,
  parentId: string | null,
  laneId?: string,
) {
  try {
    await invoke("reparent_session", { sessionId, parentId });
    // Filing into a lane with no parent: also move the placement group so
    // the chat shows up under that lane header.
    if (!parentId && laneId) {
      await invoke("move_to_workstream", {
        sessionIds: [sessionId],
        groupId: laneId,
      });
    }
  } catch (e: any) {
    connError.value = e?.message ?? String(e);
  }
}

function onArchived(id: string) {
  if (selectedId.value === id) {
    selectedId.value = null;
    selectedView.value = null;
  }
  fleet.value = fleet.value.filter((s) => s.id !== id);
}

const connClass = computed(() =>
  connected.value ? "ok" : connError.value ? "bad" : "warn",
);
</script>

<template>
  <div class="app" data-tauri-drag-region>
    <header class="header" data-tauri-drag-region>
      <span class="title">🕸 Arachne</span>
      <span
        class="conn"
        :class="{ clickable: true }"
        @click="showSettings = true"
        title="Connection settings"
      >
        <span class="dot" :class="connClass"></span>
        {{
          connected
            ? connError
              ? connError
              : `loom · ${loomUrl.replace("http://", "")}`
            : (connError ?? "connecting…")
        }}
      </span>
    </header>
    <FleetSidebar
      :fleet="fleet"
      :layout="layout"
      :selected-id="selectedId"
      :launching="launching"
      @select="selectSession"
      @launch="launchTask"
      @create-workstream="createWorkstream"
      @reparent="reparentSession"
    />
    <ThreadView
      v-if="selectedId && selectedView"
      :key="selectedId"
      :session="selectedView"
      @error="connError = $event"
      @archive="onArchived"
      @delegate="delegateFromThread"
    />
    <div v-else-if="selectedId" class="main">
      <div class="empty">
        <div class="big">🕸</div>
        <div>opening session…</div>
      </div>
    </div>
    <div v-else class="main">
      <div class="empty">
        <div class="big">🕸</div>
        <div>Select a session, or launch a new task from the sidebar.</div>
      </div>
    </div>
    <SettingsSheet
      v-if="showSettings"
      :url="loomUrl"
      :token="loomToken"
      :connected="connected"
      @close="showSettings = false"
      @save="saveSettings"
    />
  </div>
</template>
