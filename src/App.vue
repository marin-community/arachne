<script setup lang="ts">
import { ref, onMounted, computed } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import FleetSidebar from "./components/FleetSidebar.vue";
import ThreadView from "./components/ThreadView.vue";
import HomeView from "./components/HomeView.vue";
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
// URL is not a secret — localStorage is fine. The TOKEN is a credential:
// it lives in the macOS Keychain behind Tauri commands, never here (spec:
// "do not store sensitive credentials in frontend localStorage"). It's
// read once at boot into memory for the settings sheet's prefilled value.
const loomUrl = ref(localStorage.getItem("loomUrl") ?? DEFAULT_URL);
const loomToken = ref("");

// --- Boot: connect to loom (URL from settings) ------------------------------

onMounted(async () => {
  // Prefill the in-memory token from the Keychain (migration note: an old
  // build's localStorage token, if any, is stale and ignored).
  try {
    loomToken.value = (await invoke<string | null>("load_token")) ?? "";
  } catch {
    // Keychain unavailable (rare); connect can still proceed tokenless.
  }
  // Register listeners BEFORE connecting, so the initial fleet snapshot
  // emitted right after connect is never missed.
  await listen<FleetSnapshot>("loom://fleet", (event) => {
    fleet.value = event.payload.sessions;
    layout.value = event.payload.layout;
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
  // Persist the token to the Keychain (fire-and-forget: connect proceeds
  // on the in-memory copy; a failed save surfaces on next boot at worst).
  invoke("save_token", { token }).catch(() => {});
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
    // A new topic activates immediately: route through selectSession
    // so open_session runs (chat forwarder + cursor reset + fresh view),
    // not just the launch stub — otherwise the thread never streams live.
    await selectSession(view.id);
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
    // sidebar (and inherits the parent's topic) via the layout events.
  } catch (e: any) {
    connError.value = e?.message ?? String(e);
  } finally {
    launching.value = false;
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
      await invoke("move_to_group", {
        sessionIds: [sessionId],
        groupId: laneId,
      });
    }
  } catch (e: any) {
    connError.value = e?.message ?? String(e);
  }
}

async function deleteLane(laneId: string) {
  try {
    // Delete a user lane; its chats move back to the user-space Inbox.
    const inbox = userInboxId();
    if (!inbox) {
      connError.value = "no Inbox lane found to move chats into";
      return;
    }
    await invoke("delete_group", {
      groupId: laneId,
      destinationGroupId: inbox,
    });
  } catch (e: any) {
    connError.value = e?.message ?? String(e);
  }
}

function userInboxId(): string | undefined {
  for (const sp of layout.value?.spaces ?? []) {
    if (sp.system_key === "user" || sp.name === "User") {
      const g = sp.groups.find((g) => g.system_key === "inbox");
      if (g) return g.id;
    }
  }
  return undefined;
}

function onArchived(id: string) {
  if (selectedId.value === id) {
    selectedId.value = null;
    selectedView.value = null;
  }
  // Keep the archived row in the fleet: the sidebar shows archived children
  // dimmed under their leader, so a finished worker stays visible as part of
  // its topic's shape. The next fleet snapshot re-syncs status.
  for (const s of fleet.value) {
    if (s.id === id) s.status = "archived";
  }
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
      @reparent="reparentSession"
      @delete-lane="deleteLane"
    />
    <ThreadView
      v-if="selectedId && selectedView"
      :key="selectedId"
      :session="selectedView"
      @error="connError = $event"
      @archive="onArchived"
      @delegate="delegateFromThread"
    />
    <HomeView
      v-else
      :fleet="fleet"
      :selected-id="selectedId"
      @select="selectSession"
    />
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
