<script setup lang="ts">
import { ref, onMounted, computed, watch, nextTick } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import FleetSidebar from "./components/FleetSidebar.vue";
import ThreadView from "./components/ThreadView.vue";
import HomeView from "./components/HomeView.vue";
import SettingsSheet from "./components/SettingsSheet.vue";
import NewThreadSheet from "./components/NewThreadSheet.vue";
import NewTopicSheet from "./components/NewTopicSheet.vue";
import TopicInspector from "./components/TopicInspector.vue";
import type { FileAttachment } from "./attachments";
import { launchSelection, ensureLaunchConfig, launchAgents, launchProfiles, launchDefaultAgent } from "./launch";
import { clearDraftOnLaunch } from "./newTopicDraft";
import {
  readTopicThreadMemory,
  rememberTopicThread,
  resolveTopicThread,
  topicRootOf,
} from "./topic-view";

// --- Types mirroring src-tauri/src/loom.rs (snake_case wire) ---------------

interface TagView {
  key: string;
  note: string;
  value: string;
  set_at: string;
  set_by: string;
}
interface GithubStatus {
  pr_number: number;
  pr_url: string;
  pr_state: string;
  pr_title: string;
  is_draft: boolean;
  review_decision: string | null;
  checks: string | null;
}
interface BranchSummary {
  id: string;
  branch: string;
  name: string;
  title: string;
  description: string;
  goal: string;
  repo_root: string;
  github: GithubStatus | null;
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
export interface AcpUsage {
  used: number;
  size: number;
  cost: { amount: number; currency: string } | null;
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
  pending_permissions?: { request_id: string; title: string }[];
  usage?: AcpUsage | null;
  /** When the newest user message was journaled; null on older looms. */
  last_user_message_at?: string | null;
  // The topic's checkout path and whether it still exists on the server.
  // Lets the inspector's Resources view offer Open in Zed vs. Recover.
  work_dir: string;
  worktree_present: boolean;
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
  usage?: AcpUsage | null;
  work_dir: string;
  // The managed `owner/name` slug when launched against a managed repo.
  github_repo: string | null;
  term_session: string;
  // True when the session's checkout still exists on the server — archive
  // removes worktrees, so a false value gates Open-in-Zed behind recovery.
  worktree_present: boolean;
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
    // The launch-preset store shares this fetch's data (same command):
    // mirror it into the store so the picker's lists hydrate on connect.
    if (launchOptions.value) {
      launchAgents.value = launchOptions.value.agents;
      launchProfiles.value = launchOptions.value.profiles;
      launchDefaultAgent.value = launchOptions.value.default_agent;
    } else {
      await ensureLaunchConfig().catch(() => {});
    }
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
  // Selecting a topic while its composer sheet is open closes the sheet:
  // the topic's chat takes the main pane, and the sheet (which also lives
  // in grid-area main) would otherwise block it. The composer draft is
  // snapshotted (localStorage) and restored when the sheet reopens.
  showNewTopic.value = false;
  selectedTopicId.value = id;
  // The chat-first route (docs/design.md "Navigation"): the main pane is a
  // conversation whenever a Topic is open. The scoped dashboard remains an
  // explicit Overview detour — showTopicOverview — not the destination.
  openTopicChat(id);
}

// Which thread to show when a Topic opens: the current one when it belongs
// to the Topic (clicking an already-selected Topic keeps the open chat),
// otherwise the thread last opened within it while it is still available,
// else the coordinator. Pure decision logic lives in src/topic-view.ts.
async function openTopicChat(id: string) {
  const choice = resolveTopicThread({
    topicId: id,
    fleet: fleet.value,
    currentThreadId: viewMode.value === "thread" ? selectedId.value : null,
    rememberedThreadId: readTopicThreadMemory(localStorage)[id] ?? null,
  });
  if (choice.source !== "current") rememberTopicThread(localStorage, id, choice.threadId);
  await selectSession(choice.threadId);
}

// The explicit scoped dashboard (HomeView topic mode): Needs You, Working,
// Ready to Integrate, and the topic summary. Reached deliberately through
// the Overview action in the Topic/coordinator chat header — the main pane
// stays a conversation whenever a Topic is open.
function showTopicOverview(id: string) {
  selectedTopicId.value = id;
  selectedId.value = null;
  selectedView.value = null;
  viewMode.value = "topic";
}

// The explicit back-to-coordinator action (docs/design.md "Navigation"):
// from a worker thread, open the topic's coordinator chat directly.
// Unlike selectTopic/openTopicChat, this deliberately bypasses the
// remembered-thread restoration — the user asked for the coordinator,
// not for whichever thread they last opened inside this topic.
async function openCoordinatorThread(topicId: string) {
  const coordinator = fleet.value.find((session) => session.id === topicId) ?? topicRootOf(fleet.value, topicId);
  if (!coordinator) return;
  await selectSession(coordinator.id);
}

// Keep the last-opened-thread memory current while the user moves between
// threads: entering a thread inside a topic records it for that topic. A
// stale entry (thread deleted, archived, or reparented) is simply never
// restored — resolveTopicThread re-validates before using it.
watch([selectedId, viewMode, fleet], () => {
  const threadId = viewMode.value === "thread" ? selectedId.value : null;
  if (!threadId || !fleet.value.length) return;
  const topic = topicRootOf(fleet.value, threadId);
  if (topic && topic.id !== threadId) rememberTopicThread(localStorage, topic.id, threadId);
});

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

const newTopicProject = ref<{ id: string | null; name: string } | null>(null);

function openNewThread() {
  // A stale connection error from an earlier flow shouldn't read as a
  // launch failure inside the fresh sheet.
  connError.value = null;
  showNewTopic.value = false;
  newTopicProject.value = null;
  showNewThread.value = true;
}

function openNewTopic() {
  connError.value = null;
  showNewThread.value = false;
  newTopicProject.value = null;
  showNewTopic.value = true;
}

// FleetSidebar's project affordances route through here so App owns the
// sheet. Navigation itself (the project-filtered home) is a separate
// minimal handler.
function openNewTopicInProject(project: { id: string | null; name: string } | null) {
  connError.value = null;
  newTopicProject.value = project;
  showNewThread.value = false;
  showNewTopic.value = true;
}
function closeNewTopic() {
  showNewTopic.value = false;
  newTopicProject.value = null;
}
function closeNewThreadSheet() {
  showNewThread.value = false;
}

// Selecting a project heading filters the main-pane home to that project's
// topics (null = the aggregate Topics home).
const selectedProject = ref<{ id: string | null; name: string } | null>(null);
function selectProject(project: { id: string | null; name: string } | null) {
  selectedProject.value = project;
  showTopicsHome();
}

// Home button and breadcrumb: leaving home clears the project filter too
// (the aggregate Topics home spans all projects).
function onHome() {
  selectedProject.value = null;
  showTopicsHome();
}

async function launchTask(
  task: string,
  repo: string,
  meta?: { title?: string; description?: string; base?: string; mentions?: ResourceMention[]; attachments?: FileAttachment[]; profile?: string; agent?: string; model?: string; effort?: string; project?: { id: string | null; name: string } },
  completed?: (success: boolean) => void,
) {
  launching.value = true;
  try {
    const view = await invoke<SessionView>("launch_session", {
      repo,
      task,
      title: meta?.title ?? null,
      description: meta?.description ?? null,
      base: meta?.base ?? null,
      mentions: meta?.mentions ?? [],
      attachments: meta?.attachments?.map(({ name, contentBase64 }) => ({ name, contentBase64 })) ?? [],
      profile: meta?.profile || null,
      agent: meta?.agent || null,
      model: meta?.model || null,
      effort: meta?.effort || null,
      project: meta?.project ?? null,
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
// keeps the drafts for a retry. A successful launch also drops the stored
// composer draft — it became a real topic branch, so reopening the sheet
// starts fresh rather than resurrecting an already-launched draft.
function launchTopic(
  task: string,
  repo: string,
  meta?: { title?: string; description?: string; base?: string; mentions?: ResourceMention[]; attachments?: FileAttachment[]; profile?: string; agent?: string; model?: string; effort?: string; project?: { id: string | null; name: string } },
) {
  launchTask(task, repo, meta, (success) => {
    if (success) {
      closeNewTopic();
      // Unmounting the sheet snapshots its draft (onUnmounted runs on
      // this same flush); drop it a microtask later, after that snapshot
      // has landed, so reopening the sheet starts fresh.
      nextTick(() => clearDraftOnLaunch(localStorage));
    }
  });
}

// The thread sheet's launch: the sheet's draft carries attachments,
// resource mentions, and the launch config (the removed sidebar composer's
// affordances). Project preselection lives on the topic sheet — a project
// files topics, so its + opens that sheet.
function launchThread(
  task: string,
  repo: string,
  meta?: { mentions?: ResourceMention[]; attachments?: FileAttachment[]; profile?: string; agent?: string; model?: string; effort?: string },
) {
  launchTask(task, repo, meta, (success) => {
    if (success) closeNewThreadSheet();
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
  // The launch-preset picker's current selection rides along: the last
  // chosen harness/model/effort is the default for every delegation.
  const sel = launchSelection.value;
  try {
    const view = await invoke<SessionView>("delegate_task", {
      parentId,
      task,
      agent: sel.agent || null,
      model: sel.model || null,
      effort: sel.effort || null,
    });
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
        title="Topic inspector" @click="showResources = !showResources">Inspector</button>
    </header>
    <FleetSidebar
      :fleet="fleet"
      :layout="layout"
      :selected-id="selectedId ?? selectedTopicId"
      :show-new-thread="showNewThread"
      :show-new-topic="showNewTopic"
      @select="selectSession"
      @select-topic="selectTopic"
      @update-topic="updateTopic"
      @new-thread="openNewThread"
      @new-topic="openNewTopic"
      @new-topic-in-project="openNewTopicInProject"
      @select-project="selectProject"
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
      :project="newTopicProject"
      @close="closeNewTopic"
      @launch="launchTopic"
    />
    <NewThreadSheet
      v-else-if="showNewThread"
      :fleet="fleet"
      :launching="launching"
      :error="connError"
      :launch-options="launchOptions"
      @close="closeNewThreadSheet"
      @launch="launchThread"
    />
    <ThreadView
      v-else-if="viewMode === 'thread' && selectedId && selectedView"
      :key="selectedId"
      :session="selectedView"
      :topic="selectedTopic"
      :fleet="fleet"
      :launch-options="launchOptions"
      :loom-url="loomUrl"
      @error="connError = $event"
      @archive="onArchived"
      @delegate="delegateFromThread"
      @handoff="selectSession"
      @refresh="selectSession"
      @open-topic="selectTopic"
      @overview="showTopicOverview"
      @open-coordinator="openCoordinatorThread"
      @home="showTopicsHome"
    />
    <HomeView
      v-else
      :key="viewMode === 'topic' ? selectedTopicId ?? 'topic' : 'home'"
      :fleet="fleet"
      :topic="viewMode === 'topic' ? selectedTopic : null"
      :project="selectedProject"
      :layout="layout"
      @select="selectSession"
      @new-thread="openNewThread"
      @new-topic="openNewTopic"
      @open-zed="openTopicInZed"
      @home="onHome"
      @error="connError = $event"
    />
    <TopicInspector v-if="!showNewThread && viewMode !== 'home' && selectedTopic && showResources" :topic="selectedTopic" :fleet="fleet" :selected-id="selectedId"
      @close="showResources = false" @error="connError = $event" @select="selectSession" @new-thread="openNewThread" />
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
