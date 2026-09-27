<script setup lang="ts">
import { ref, onMounted, computed } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import FleetSidebar from "./components/FleetSidebar.vue";
import ThreadView from "./components/ThreadView.vue";
import HomeView from "./components/HomeView.vue";
import SettingsSheet from "./components/SettingsSheet.vue";
import NewThreadSheet from "./components/NewThreadSheet.vue";
import NewTopicSheet from "./components/NewTopicSheet.vue";
import ResourcePanel from "./components/ResourcePanel.vue";
import type { FileAttachment } from "./attachments";

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
  github?: {
    pr_number: number;
    pr_url: string;
    pr_state: string;
    checks: string | null;
    review_decision: string | null;
  } | null;
  github_pr?: number | null;
  tags: TagView[];
  title_provenance?: string;
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
export interface ResourceMention { topicId: string; resourceId: string }
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
  github_repo: string | null;
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
export interface LaunchOptions {
  profiles: { name: string; description: string; agent_kind: string; model: string; effort: string; class: string }[];
  agents: {
    kind: string;
    label: string;
    models: { id: string; label: string }[];
    efforts: { id: string; label: string }[];
    accepts_raw_model: boolean;
  }[];
  default_agent: string;
}

// --- State ----------------------------------------------------------------

const DEFAULT_URL = "http://127.0.0.1:7878";
const connected = ref(false);
const connError = ref<string | null>(null);
const launching = ref(false);
const fleet = ref<SessionSummary[]>([]);
const layout = ref<SessionLayout | null>(null);
const launchOptions = ref<LaunchOptions | null>(null);
const selectedId = ref<string | null>(null);
const selectedTopicId = ref<string | null>(null);
const viewMode = ref<"home" | "topic" | "thread">("home");
const selectedView = ref<SessionView | null>(null);
// The new-thread composer: a sheet over the main panel, not a modal and
// not the sidebar. Takes over while open; a launch closes it and opens
// the live thread (launchTask → selectSession).
const showNewThread = ref(false);
// The topic composer: same main-panel takeover as the thread sheet. A
// topic is a leader chat with title/description metadata, so its launch
// path (launchTopic) passes those through to launch_session.
const showNewTopic = ref(false);
const showSettings = ref(false);
const showResources = ref(true);
const settingsError = ref<string | null>(null);
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
    connected.value = true;
    connError.value = null;
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
    launchOptions.value = await invoke<LaunchOptions>("launch_options").catch(() => null);
  } catch (e: any) {
    connected.value = false;
    connError.value = e?.message ?? String(e);
    launchOptions.value = null;
  }
}

async function saveSettings(url: string, token: string) {
  settingsError.value = null;
  try {
    await invoke("save_token", { token });
  } catch (e: any) {
    settingsError.value = `Could not save the token in Keychain: ${e?.message ?? String(e)}`;
    return;
  }
  loomUrl.value = url;
  loomToken.value = token;
  localStorage.setItem("loomUrl", url);
  await connect(url, token || null);
  if (connected.value) showSettings.value = false;
  else settingsError.value = connError.value;
}

// --- Actions ----------------------------------------------------------------

async function selectSession(id: string) {
  viewMode.value = "thread";
  selectedId.value = id;
  // Drop the stale view immediately: ThreadView is keyed by selectedId and
  // remounts the moment it changes — if the old view were still here, its
  // onMounted would fetch the PREVIOUS session's chat, and nothing would
  // re-run when the fresh view arrives (clicked row N, saw row N±1's thread).
  selectedView.value = null;
  try {
    const view = await invoke<SessionView>("open_session", { id });
    if (viewMode.value === "thread" && selectedId.value === id) selectedView.value = view;
  } catch (e: any) {
    if (viewMode.value === "thread" && selectedId.value === id)
      connError.value = e?.message ?? String(e);
  }
}

function selectTopic(id: string) {
  selectedTopicId.value = id;
  selectedId.value = null;
  selectedView.value = null;
  viewMode.value = "topic";
}

function showTopicsHome() {
  selectedId.value = null;
  selectedTopicId.value = null;
  selectedView.value = null;
  viewMode.value = "home";
}

async function openTopicInZed(id: string) {
  try {
    await invoke("open_in_zed", { id });
  } catch (e: any) {
    connError.value = e?.message ?? String(e);
  }
}

function openNewThread() {
  // A stale connection error from an earlier flow shouldn't read as a
  // launch failure inside the fresh sheet.
  connError.value = null;
  showNewTopic.value = false;
  showNewThread.value = true;
}

function openNewTopic() {
  connError.value = null;
  showNewThread.value = false;
  showNewTopic.value = true;
}

async function launchTask(
  task: string,
  repo: string,
  meta?: { title?: string; description?: string; oneOff?: boolean; mentions?: ResourceMention[]; attachments?: FileAttachment[]; profile?: string; agent?: string; model?: string; effort?: string },
  completed?: (success: boolean) => void,
) {
  launching.value = true;
  try {
    const view = await invoke<SessionView>("launch_session", {
      repo,
      task,
      title: meta?.title ?? null,
      description: meta?.description ?? null,
      oneOff: meta?.oneOff ?? false,
      mentions: meta?.mentions ?? [],
      attachments: meta?.attachments?.map(({ name, contentBase64 }) => ({ name, contentBase64 })) ?? [],
      profile: meta?.profile || null,
      agent: meta?.agent || null,
      model: meta?.model || null,
      effort: meta?.effort || null,
    });
    // A new topic activates immediately: route through selectSession
    // so open_session runs (chat forwarder + cursor reset + fresh view),
    // not just the launch stub — otherwise the thread never streams live.
    showNewThread.value = false;
    await selectSession(view.id);
    completed?.(true);
  } catch (e: any) {
    // Keep the sheet open with the draft intact: a failed launch
    // (bad repo, loom down) is one edit away from a retry, not a blank form.
    connError.value = e?.message ?? String(e);
    completed?.(false);
  } finally {
    launching.value = false;
  }
}

// The topic sheet's launch: same signature as a thread launch — the sheet
// owns richer card metadata (title/description) plus attachments and
// resource mentions, all routed through launchTask so the wiring stays in
// one place. Success closes the sheet and opens the live thread; failure
// keeps the drafts for a retry.
function launchTopic(
  task: string,
  repo: string,
  meta?: { title?: string; description?: string; mentions?: ResourceMention[]; attachments?: FileAttachment[]; profile?: string; agent?: string; model?: string; effort?: string },
) {
  launchTask(task, repo, meta, (success) => {
    if (success) showNewTopic.value = false;
  });
}

// Edit a topic's card: title (CAS-fenced against the value the card last
// rendered), goal, and description. The command re-emits the fleet
// snapshot itself — loom publishes no SSE event for these fields.
async function updateTopic(
  id: string,
  fields: { title?: string; goal?: string; description?: string },
  expected?: { title: string; provenance: string },
) {
  try {
    await invoke("update_session", {
      session: id,
      title: fields.title ?? null,
      goal: fields.goal ?? null,
      description: fields.description ?? null,
      expectedTitle: expected?.title ?? null,
      expectedTitleProvenance: expected?.provenance ?? null,
    });
  } catch (e: any) {
    connError.value = e?.message ?? String(e);
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

async function onArchived(id: string) {
  // The backend call lives here (not in the emitting component) so both
  // the thread header and the sidebar row hover button share one path.
  try {
    await invoke("archive_session", { id });
  } catch (e: any) {
    connError.value = e?.message ?? String(e);
    return;
  }
  if (selectedId.value === id) {
    showTopicsHome();
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

const selectedTopic = computed(() => {
  let node = fleet.value.find((session) => session.id === (selectedId.value ?? selectedTopicId.value));
  if (!node) return null;
  const seen = new Set<string>();
  while (node && !seen.has(node.id)) {
    seen.add(node.id);
    const parent: SessionSummary | undefined = fleet.value.find((session) =>
      session.id === node?.parent_session_id || session.branch.id === node?.parent_id,
    );
    if (!parent) break;
    node = parent;
  }
  return node;
});
</script>

<template>
  <div class="app" :class="{ 'with-resources': !showNewThread && viewMode !== 'home' && !!selectedTopic && showResources }" data-tauri-drag-region>
    <header class="header" data-tauri-drag-region>
      <button class="title home-link" title="Show Topics home" @click="showTopicsHome">🕸 Arachne</button>
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
      <button v-if="viewMode !== 'home' && selectedTopic" class="header-resources" :aria-pressed="showResources"
        @click="showResources = !showResources">Resources</button>
    </header>
    <FleetSidebar
      :fleet="fleet"
      :layout="layout"
      :selected-id="selectedId ?? selectedTopicId"
      :show-new-thread="showNewThread"
      :show-new-topic="showNewTopic"
      :launching="launching"
      :launch-options="launchOptions"
      @select="selectSession"
      @select-topic="selectTopic"
      @launch="launchTask"
      @update-topic="updateTopic"
      @new-thread="openNewThread"
      @new-topic="openNewTopic"
      @reparent="reparentSession"
      @delete-lane="deleteLane"
      @archive="onArchived"
    />
    <NewTopicSheet
      v-if="showNewTopic"
      :fleet="fleet"
      :launching="launching"
      :error="connError"
      :launch-options="launchOptions"
      @close="showNewTopic = false"
      @launch="launchTopic"
    />
    <NewThreadSheet
      v-else-if="showNewThread"
      :launching="launching"
      :error="connError"
      @close="showNewThread = false"
      @launch="launchTask"
    />
    <ThreadView
      v-else-if="viewMode === 'thread' && selectedId && selectedView"
      :key="selectedId"
      :session="selectedView"
      :topic="selectedTopic"
      :fleet="fleet"
      :launch-options="launchOptions"
      @error="connError = $event"
      @archive="onArchived"
      @delegate="delegateFromThread"
      @handoff="selectSession"
      @refresh="selectSession"
      @open-topic="selectTopic"
      @home="showTopicsHome"
    />
    <HomeView
      v-else
      :key="viewMode === 'topic' ? selectedTopicId ?? 'topic' : 'home'"
      :fleet="fleet"
      :topic="viewMode === 'topic' ? selectedTopic : null"
      @select="selectSession"
      @new-thread="openNewThread"
      @new-topic="openNewTopic"
      @open-zed="openTopicInZed"
      @home="showTopicsHome"
    />
    <ResourcePanel v-if="!showNewThread && viewMode !== 'home' && selectedTopic && showResources" :topic="selectedTopic"
      @close="showResources = false" @error="connError = $event" />
    <SettingsSheet
      v-if="showSettings"
      :url="loomUrl"
      :token="loomToken"
      :connected="connected"
      :error="settingsError"
      @close="showSettings = false"
      @save="saveSettings"
    />
  </div>
</template>
