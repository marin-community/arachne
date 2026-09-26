<script setup lang="ts">
import { ref, onMounted, computed } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import FleetSidebar from "./components/FleetSidebar.vue";
import ThreadView from "./components/ThreadView.vue";

// --- Types mirroring src-tauri/src/loom.rs -------------------------------

interface TagView { key: string; note: string; value: string; set_at: string; set_by: string }
interface BranchSummary {
  id: string; branch: string; name: string; title: string;
  description: string; goal: string; repo_root: string; tags: TagView[];
}
interface Placement {
  space_id: string | null; space_name: string | null;
  group_id: string | null; group_name: string | null;
  group_system_key: string | null; rank: number | null; session_id: string | null;
}
export interface SessionSummary {
  id: string; status: string; profile: string; class: string; origin: string;
  created_by: string | null; created_at: string; last_activity_at: string;
  branch: BranchSummary; placement: Placement | null;
  github_repo: string | null; parent_id: string | null; parent_session_id: string | null;
}
export interface SessionView {
  id: string; status: string; profile: string; class: string; origin: string;
  agent_kind: string; model: string; effort: string; protocol: string;
  work_dir: string; term_session: string; turn_count: number;
  created_by: string | null; created_at: string; last_activity_at: string;
  branch: BranchSummary; placement: Placement | null;
}

// --- State ----------------------------------------------------------------

const connected = ref(false);
const connError = ref<string | null>(null);
const fleet = ref<SessionSummary[]>([]);
const selectedId = ref<string | null>(null);
const selectedView = ref<SessionView | null>(null);

// --- Boot: connect to the local loom --------------------------------------

const LOOM_URL = "http://127.0.0.1:7878";

onMounted(async () => {
  // Register listeners BEFORE connecting, so the initial fleet snapshot
  // emitted right after connect is never missed.
  await listen<SessionSummary[]>("loom://fleet", (event) => {
    fleet.value = event.payload;
  });
  await listen("loom://launched", (event) => {
    const view = event.payload as SessionView;
    selectedId.value = view.id;
    selectedView.value = view;
  });
  // Transient API errors (a session mid-archive, etc.) surface in the header.
  await listen("loom://error", (event) => {
    const err = event.payload as { message: string; unreachable: boolean };
    if (err.unreachable) {
      connected.value = false;
      connError.value = err.message;
    }
  });

  try {
    await invoke("connect", { baseUrl: LOOM_URL, token: null });
    connected.value = true;
    connError.value = null;
  } catch (e: any) {
    connError.value = e?.message ?? String(e);
  }
});

// --- Actions ----------------------------------------------------------------

async function selectSession(id: string) {
  selectedId.value = id;
  try {
    selectedView.value = await invoke<SessionView>("open_session", { id });
  } catch (e: any) {
    connError.value = e?.message ?? String(e);
  }
}

async function launchTask(task: string, repo: string) {
  try {
    const view = await invoke<SessionView>("launch_session", { repo, task });
    selectedId.value = view.id;
    selectedView.value = view;
  } catch (e: any) {
    connError.value = e?.message ?? String(e);
  }
}

const connClass = computed(() =>
  connected.value ? "ok" : connError.value ? "bad" : "warn"
);
</script>

<template>
  <div class="app" data-tauri-drag-region>
    <header class="header" data-tauri-drag-region>
      <span class="title">🕸 Arachne</span>
      <span class="conn">
        <span class="dot" :class="connClass"></span>
        {{ connected ? "loom · 127.0.0.1:7878" : connError ?? "connecting…" }}
      </span>
    </header>
    <FleetSidebar
      :fleet="fleet"
      :selected-id="selectedId"
      @select="selectSession"
      @launch="launchTask"
    />
    <ThreadView
      v-if="selectedId && selectedView"
      :key="selectedId"
      :session="selectedView"
      @error="connError = $event"
    />
    <div v-else class="main">
      <div class="empty">
        <div class="big">🕸</div>
        <div>Select a session, or launch a new task from the sidebar.</div>
      </div>
    </div>
  </div>
</template>
