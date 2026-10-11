<script setup lang="ts">
import { ref, onMounted, onUnmounted, computed, watch, nextTick, provide } from "vue";
import { invoke as rawInvoke } from "@tauri-apps/api/core";
import { backendInvoke, useBackendInvoke } from "./backendInvoke";
import { backendOf, splitScopedId } from "./backendIdentity";
import { normalizeServerUrl, readServerProfiles } from "./serverProfiles";
import { registeredBackends, registerBackend, combinedFleet, combinedLayout, acceptSnapshot, markUnavailable, cacheProjectLayout } from "./backendFleet";
import { listen } from "@tauri-apps/api/event";
import CommandPalette from "./components/CommandPalette.vue";
import AttentionBanner from "./components/AttentionBanner.vue";
import { isStandalone } from "./threadKind";
import FleetSidebar from "./components/FleetSidebar.vue";
import ThreadView from "./components/ThreadView.vue";
import HomeView from "./components/HomeView.vue";
import SettingsSheet from "./components/SettingsSheet.vue";
import NewThreadSheet from "./components/NewThreadSheet.vue";
import NewTrackChat from "./components/NewTrackChat.vue";
import ProjectSheet from "./components/ProjectSheet.vue";
import TopicInspector from "./components/TopicInspector.vue";
import type { ArtifactTarget } from "./artifactLinks";
import type { FileAttachment } from "./attachments";
import { launchSelectionFor } from "./launch";
import { clearSubmittedDraft, draftStorageKey, initializeDraftStorage } from "./newTopicDraft";
import { isTrackCoordinator, launchTopicBranch } from "./trackPolicy";
import {
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
  server_url: string;
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

initializeDraftStorage(localStorage);

const DEFAULT_URL = "http://127.0.0.1:7878";
const connected = computed(() => backends.value.some(backend => backend.available));
const connError = ref<string | null>(null);
const unarchivingIds = ref(new Set<string>());
const pendingLaunches = ref(new Set<string>());
const launching = computed(() => pendingLaunches.value.has(operationUrl.value));
const fleet = computed(() => combinedFleet(backends.value));
const layout = computed(() => combinedLayout(backends.value));
const launchOptions = computed(() => backends.value.find(backend => backend.url === operationUrl.value)?.launchOptions ?? null);
const selectedId = ref<string | null>(null);
const selectedTopicId = ref<string | null>(null);
const viewMode = ref<"home" | "topic" | "thread">("home");
const selectedView = ref<SessionView | null>(null);
const selectedSummarySnapshot = ref<SessionSummary | null>(null);
// The new-thread composer: a sheet over the main panel, not a modal and
// not the sidebar. Takes over while open; a launch closes it and opens
// the live thread (launchTask → selectSession).
const showNewThread = ref(false);
// The new-track chat: same main-panel takeover as the thread sheet, but
// chat-first — it looks like the thread it will become, with the setup
// choices (title, repo/base, project bindings) in the conversation area
// and the first send launching the track (launchTopic).
const showNewTopic = ref(false);
const showNewProject = ref(false);
const showSettings = ref(false);
const inspectorExpanded = ref(false);
const reviewRequest = ref<{ sessionId: string } | null>(null);
function openReview(sessionId: string) {
  reviewRequest.value = { sessionId };
  artifactRequest.value = null;
  showResources.value = true;
}
const artifactRequest = ref<ArtifactTarget | null>(null);
function openArtifact(target: ArtifactTarget) {
  artifactRequest.value = { ...target };
  showResources.value = true;
  reviewRequest.value = null;
}
const showResources = ref(localStorage.getItem("arachne.inspector") !== "false");
const landingRequest = ref(0);
watch(showResources, value => localStorage.setItem("arachne.inspector", String(value)));
const showSearch = ref(false);
const settingsSaving = ref(false);
const threadView = ref<InstanceType<typeof ThreadView> | null>(null);
const newTrackChat = ref<InstanceType<typeof NewTrackChat> | null>(null);
let connectionGeneration = 0;
const composerGeneration = ref(0);
const topicDraftOwners = new Map<string, number>();
function closeLaunchers() { composerGeneration.value++; showNewTopic.value = false; showNewThread.value = false; showNewProject.value = false; projectResourcesProject.value = null; }
async function openWorker() {
  if (!selectedView.value && selectedTopic.value) await selectSession(selectedTopic.value.id);
  inspectorExpanded.value = false;
  await nextTick();
  await threadView.value?.openDelegate();
}
function command(action: string) {
  if (action === "thread") openNewThread();
  else if (action === "track") openNewTopic();
  else if (action === "home") onHome();
  else if (action === "delegate") openWorker();
  else if (action === "inspector") showResources.value = !showResources.value;
  else if (action === "settings") showSettings.value = true;
}
function onShortcut(event: KeyboardEvent) {
  if (event.defaultPrevented) return;
  if (event.key === "Escape") {
    if (showSearch.value) showSearch.value = false;
    else if (showSettings.value && !settingsSaving.value) showSettings.value = false;
    else if (showNewTopic.value || showNewThread.value || showNewProject.value || projectResourcesProject.value) closeLaunchers();
    else return;
    event.preventDefault(); return;
  }
  if (!event.metaKey && !event.ctrlKey) return;
  const key = event.key.toLowerCase();
  if (key === "k") { showSearch.value = !showSearch.value; event.preventDefault(); }
  else if (key === ",") { showSettings.value = true; event.preventDefault(); }
  else if (key === "n") { command(event.shiftKey ? "track" : "thread"); event.preventDefault(); }
  else if (event.shiftKey && ["h", "d", "e"].includes(key)) {
    command(({h: "home", d: "delegate", e: "inspector"} as Record<string, string>)[key]); event.preventDefault();
  }
}
onMounted(() => window.addEventListener("keydown", onShortcut));
onUnmounted(() => window.removeEventListener("keydown", onShortcut));
const settingsError = ref<string | null>(null);
// URL is not a secret — localStorage is fine. The TOKEN is a credential:
// it lives in the macOS Keychain behind Tauri commands, never here (spec:
// "do not store sensitive credentials in frontend localStorage"). It's
// read once at boot into memory for the settings sheet's prefilled value.
let initialUrl = DEFAULT_URL;
try { initialUrl = normalizeServerUrl(localStorage.getItem("loomUrl") ?? DEFAULT_URL); } catch { /* Invalid legacy URL. */ }
const loomUrl = ref(initialUrl); // The default for new projects/unfiled tracks.
const backends = ref(registeredBackends(localStorage, initialUrl));
const composerUrl = ref(initialUrl);
const loomToken = ref("");
const operationUrl = computed(() => {
  if (showNewTopic.value || showNewThread.value || showNewProject.value) return composerUrl.value;
  return backendOf(projectResourcesProject.value?.id) || backendOf(selectedId.value) || backendOf(selectedTopicId.value) || backendOf(selectedProject.value?.id) || loomUrl.value;
});
provide("backendUrl", operationUrl);
const currentBackend = computed(() => backends.value.find(backend => backend.url === operationUrl.value));
const currentAvailable = computed(() => currentBackend.value?.available ?? false);
const composerFleet = computed(() => currentBackend.value?.available ? currentBackend.value.sessions : []);
const composerLayout = computed(() => currentBackend.value?.layout ?? null);
const invoke = useBackendInvoke(operationUrl);
function reportActionError(url: string | null, error: any, publish = true) {
  const backend = backends.value.find(backend => backend.url === url);
  const message = error?.message ?? String(error);
  if (backend && error?.unreachable) markUnavailable(backend, message);
  if (publish && (!url || operationUrl.value === url)) connError.value = backend ? `${backend.name}: ${message}` : message;
}


let appDisposed = false;
const eventStops: (() => void)[] = [];
onUnmounted(() => { appDisposed = true; eventStops.forEach(stop => stop()); });
async function subscribe<T>(name: string, receive: (event: { payload: T }) => void) {
  const stop = await listen<T>(name, receive);
  if (appDisposed) stop(); else eventStops.push(stop);
}
onMounted(async () => {
  await subscribe<FleetSnapshot>("loom://fleet", event => {
    const url = normalizeServerUrl(event.payload.server_url);
    const backend = backends.value.find(backend => backend.url === url);
    if (!backend) return;
    acceptSnapshot(backend, event.payload.sessions, event.payload.layout);
    cacheProjectLayout(localStorage, url, event.payload.layout);
  });
  await subscribe("loom://error", event => {
    const error = event.payload as { message: string; unreachable: boolean; unavailable?: boolean; server_url?: string };
    if (!error.server_url) return;
    const backend = backends.value.find(backend => backend.url === normalizeServerUrl(error.server_url!));
    if (backend && (error.unavailable || error.unreachable)) markUnavailable(backend, error.message);
    else if (backend && backend.url === operationUrl.value) connError.value = `${backend.name}: ${error.message}`;
  });
  await Promise.allSettled(backends.value.map(backend => connect(backend.url)));
});

const connectRequests = new Map<string, number>();
async function connect(url: string, suppliedToken?: string | null) {
  const backend = backends.value.find(backend => backend.url === url);
  if (!backend) return;
  const request = (connectRequests.get(url) ?? 0) + 1;
  connectRequests.set(url, request);
  backend.connecting = true;
  try {
    const token = suppliedToken !== undefined ? suppliedToken : await rawInvoke<string | null>("load_token", { baseUrl: url, migrateLegacy: url === initialUrl }).catch(() => null);
    // Credential lookup can outlive a newer settings/retry request. Retire it
    // before native connect reserves and installs a client with stale credentials.
    if (appDisposed || request !== connectRequests.get(url)) return;
    await rawInvoke("connect", { baseUrl: url, token });
    if (request !== connectRequests.get(url)) return;
    // The initial pushed snapshot may race the command reply. Request a
    // confirmed snapshot so availability always carries a coherent fleet.
    const snapshot = await rawInvoke<FleetSnapshot>("refresh_fleet", { baseUrl: url });
    if (request !== connectRequests.get(url)) return;
    acceptSnapshot(backend, snapshot.sessions, snapshot.layout);
    cacheProjectLayout(localStorage, url, snapshot.layout);
    const options = await rawInvoke<LaunchOptions>("launch_options", { baseUrl: url }).catch(() => null);
    if (request === connectRequests.get(url)) backend.launchOptions = options;
  } catch (error: any) {
    if (request === connectRequests.get(url)) markUnavailable(backend, error?.message ?? String(error));
  } finally {
    if (request === connectRequests.get(url)) backend.connecting = false;
  }
}
async function retryBackend(url: string) {
  await connect(url);
  if (backendOf(selectedId.value) === url && currentAvailable.value && selectedId.value) await selectSession(selectedId.value);
}
function changeComposerBackend(url: string) {
  if (launching.value) return;
  composerGeneration.value++;
  connectionGeneration++;
  composerUrl.value = url;
  if (showNewTopic.value) topicDraftOwners.set(url, composerGeneration.value);
  newTopicProject.value = null;
  connError.value = null;
}
function onProjectLayout(next: SessionLayout) {
  const backend = currentBackend.value;
  if (!backend) return;
  backend.layout = next;
  // Persist the confirmed server layout by removing UI qualification.
  const raw = JSON.parse(JSON.stringify(next), (_key, value) => typeof value === "string" ? splitScopedId(value)?.id ?? value : value);
  cacheProjectLayout(localStorage, backend.url, raw);
}
async function saveSettings(url: string, token: string) {
  if (settingsSaving.value) return;
  settingsSaving.value = true;
  settingsError.value = null;
  try {
    url = normalizeServerUrl(url);
    const name = readServerProfiles(localStorage).find(profile => profile.url === url)?.name || new URL(url).host;
    const backend = registerBackend(localStorage, backends.value, { url, name });
    await connect(url, token || null);
    if (!backend.available) { settingsError.value = backend.error; return; }
    await rawInvoke("save_token", { baseUrl: url, token });
    // Default changes affect only future creation. All existing projects,
    // selections and requests keep their own backend association.
    loomUrl.value = url;
    localStorage.setItem("loomUrl", url);
    if (backendOf(selectedId.value) === url && selectedId.value) await selectSession(selectedId.value);
    showSettings.value = false;
  } catch (error: any) {
    settingsError.value = error?.message ?? String(error);
  } finally { settingsSaving.value = false; }
}

// --- Actions ----------------------------------------------------------------

async function selectSession(id: string, launchedView?: SessionView, launcher = composerGeneration.value) {
  if (!backends.value.find(backend => backend.url === backendOf(id))?.available) return;
  if (!launchedView) closeLaunchers();
  const generation = ++connectionGeneration;
  viewMode.value = "thread";
  selectedId.value = id;
  selectedSummarySnapshot.value = fleet.value.find(session => session.id === id)
    ?? (selectedSummarySnapshot.value?.id === id ? selectedSummarySnapshot.value : null);
  // Drop the stale view immediately: ThreadView is keyed by selectedId and
  // remounts the moment it changes — if the old view were still here, its
  // onMounted would fetch the PREVIOUS session's chat, and nothing would
  // re-run when the fresh view arrives (clicked row N, saw row N±1's thread).
  selectedView.value = null;
  try {
    const view = await invoke<SessionView>("open_session", { id });
    if (generation === connectionGeneration && viewMode.value === "thread" && selectedId.value === id) {
      selectedView.value = view;
      if (launchedView && launcher === composerGeneration.value) closeLaunchers();
    }
  } catch (e: any) {
    if (generation === connectionGeneration && viewMode.value === "thread" && selectedId.value === id) {
      connError.value = e?.message ?? String(e);
      if (launchedView) {
        // Creation succeeded even if activating its stream failed. Show the
        // existing track with the error instead of a launch retry.
        selectedView.value = launchedView;
        if (launcher === composerGeneration.value) closeLaunchers();
      }
    }
  }
}

// Inspector row/review clicks on the active thread must retain its composer.
function selectInspectorSession(id: string) {
  if (viewMode.value === "thread" && selectedId.value === id && selectedView.value) return;
  void selectSession(id);
}

function selectTopic(id: string) {
  // Selecting a topic while its composer sheet is open closes the sheet:
  // the topic's chat takes the main pane, and the sheet (which also lives
  // in grid-area main) would otherwise block it. The composer draft is
  // snapshotted (localStorage) and restored when the sheet reopens.
  closeLaunchers();
  selectedTopicId.value = id;
  // The chat-first route (docs/design.md "Navigation"): the main pane is a
  // conversation whenever a Topic is open. The scoped dashboard remains an
  // explicit Overview detour — showTopicOverview — not the destination.
  openTopicChat(id);
}

// Which thread to show when a Topic opens: its coordinator, always —
// clicking a Topic means the Topic's chat. The only thing kept is an
// already-open coordinator (clicking an already-selected Topic is a
// no-op, not a refetch); an open worker of that Topic resolves to the
// coordinator too, since the worker is already on screen and the click
// asked for the Topic. Pure decision logic lives in src/topic-view.ts.
async function openTopicChat(id: string) {
  const choice = resolveTopicThread({
    topicId: id,
    currentThreadId: viewMode.value === "thread" ? selectedId.value : null,
  });
  // Already there: no refetch, no remount.
  if (choice.source === "current") return;
  await selectSession(choice.threadId);
}

// The explicit scoped dashboard (HomeView topic mode): Needs You, Working,
// Reported ready to Land, and the topic summary. Reached deliberately through
// the Overview action in the Topic/coordinator chat header — the main pane
// stays a conversation whenever a Topic is open.
function showTopicOverview(id: string) {
  connectionGeneration++;
  closeLaunchers();
  selectedTopicId.value = id;
  selectedId.value = null;
  selectedView.value = null;
  viewMode.value = "topic";
}

// The explicit back-to-coordinator action (docs/design.md "Navigation"):
// from a worker thread, open the topic's coordinator chat directly.
// selectTopic lands on the same chat now; this stays as the header
// action's direct path.
async function openCoordinatorThread(topicId: string) {
  const coordinator = fleet.value.find((session) => session.id === topicId) ?? topicRootOf(fleet.value, topicId);
  if (!coordinator) return;
  await selectSession(coordinator.id);
}

function showTopicsHome() {
  connectionGeneration++;
  closeLaunchers();
  selectedId.value = null;
  selectedTopicId.value = null;
  selectedView.value = null;
  viewMode.value = "home";
}

async function openTopicInZed(id: string) {
  try {
    await invoke("open_in_zed", { id });
  } catch (e: any) {
    reportActionError(backendOf(id), e);
  }
}

const newTopicProject = ref<{ id: string | null; name: string } | null>(null);

function openNewThread() {
  connectionGeneration++;
  composerUrl.value = loomUrl.value;
  // A stale connection error from an earlier flow shouldn't read as a
  // launch failure inside the fresh sheet.
  connError.value = null;
  closeLaunchers();
  newTopicProject.value = null;
  showNewThread.value = true;
}

function openNewTopic() {
  connectionGeneration++;
  composerUrl.value = loomUrl.value;
  connError.value = null;
  closeLaunchers();
  topicDraftOwners.set(composerUrl.value, composerGeneration.value);
  newTopicProject.value = null;
  showNewTopic.value = true;
}

// FleetSidebar's project affordances route through here so App owns the
// sheet. Navigation itself (the project-filtered home) is a separate
// minimal handler.
function openNewTopicInProject(project: { id: string | null; name: string } | null) {
  const url = backendOf(project?.id) || loomUrl.value;
  if (!backends.value.find(backend => backend.url === url)?.available) return;
  connectionGeneration++;
  composerUrl.value = url;
  connError.value = null;
  closeLaunchers();
  topicDraftOwners.set(url, composerGeneration.value);
  newTopicProject.value = project;
  showNewTopic.value = true;
}
function closeNewTopic() {
  composerGeneration.value++;
  showNewTopic.value = false;
  newTopicProject.value = null;
}
function closeNewThreadSheet() {
  composerGeneration.value++;
  showNewThread.value = false;
}

// The project heading's ⚙ button: manage that project's resource bindings
// (what new topics inherit) without creating a topic first.
const projectResourcesProject = ref<{ id: string; name: string } | null>(null);
function openNewProject() {
  connectionGeneration++;
  composerUrl.value = loomUrl.value;
  connError.value = null;
  closeLaunchers();
  newTopicProject.value = null;
  showNewProject.value = true;
}
function onProjectCreated(project: { id: string; name: string }) {
  showTopicsHome();
  selectedProject.value = project;
  projectResourcesProject.value = project;
}
function openProjectResources(project: { id: string | null; name: string }) {
  // Only real projects have bindings — Ungrouped (null id) never opens
  // this sheet; the sidebar only emits for named headings anyway.
  if (!project.id || !backends.value.find(backend => backend.url === backendOf(project.id))?.available) return;
  connError.value = null;
  closeLaunchers();
  newTopicProject.value = null;
  projectResourcesProject.value = { id: project.id, name: project.name };
}
function closeProjectResources() {
  composerGeneration.value++;
  showNewProject.value = false;
  projectResourcesProject.value = null;
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
  meta?: { topicBranch?: string; title?: string; description?: string; base?: string; mentions?: ResourceMention[]; attachments?: FileAttachment[]; profile?: string; agent?: string; model?: string; effort?: string; project?: { id: string | null; name: string }; hiddenBindingKeys?: string[]; asTrack?: boolean },
  completed?: (success: boolean, currentComposer: boolean) => void,
) {
  if (!currentAvailable.value) { connError.value = "This project’s backend is unavailable. Reconnect before launching."; completed?.(false, true); return; }
  const launchUrl = operationUrl.value;
  const navigation = connectionGeneration;
  const composer = composerGeneration.value;
  if (pendingLaunches.value.has(launchUrl)) return;
  pendingLaunches.value.add(launchUrl);
  try {
    const view = await backendInvoke<SessionView>("launch_session", {
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
      // Unchecked project bindings → the topic's initial hides (design.md:
      // the creation form overrides the project's defaults).
      hiddenBindingKeys: meta?.hiddenBindingKeys ?? [],
      asTrack: meta?.asTrack ?? false,
      topicBranch: launchTopicBranch(meta?.topicBranch, meta?.asTrack ?? false),
    }, launchUrl);
    // Keep the submitted composer painted until its live view is ready.
    let currentComposer = composer === composerGeneration.value && navigation === connectionGeneration && operationUrl.value === launchUrl;
    if (currentComposer) {
      const selection = selectSession(view.id, view, composer);
      const selectedNavigation = connectionGeneration;
      await selection;
      currentComposer = connectionGeneration === selectedNavigation && composerGeneration.value === composer + 1 && selectedId.value === view.id;
    }
    completed?.(true, currentComposer);
  } catch (e: any) {
    // Keep the sheet open with the draft intact: a failed launch
    // (bad repo, loom down) is one edit away from a retry, not a blank form.
    const currentComposer = composer === composerGeneration.value && navigation === connectionGeneration;
    reportActionError(launchUrl, e, currentComposer);
    completed?.(false, currentComposer);
  } finally {
    pendingLaunches.value.delete(launchUrl);
  }
}

// The new-track chat's launch: the composer's first send launches the
// track with the setup card's metadata (title/description, repo/base,
// project bindings, launch config) plus attachments and resource
// mentions, all routed through launchTask so the wiring stays in one
// place. Success swaps this view for the live thread; failure keeps the
// chat and its drafts for a retry. A successful launch also drops the
// stored composer draft — it became a real topic branch, so reopening
// the chat starts fresh rather than resurrecting an already-launched
// draft.
function launchTopic(
  task: string,
  repo: string,
  meta?: { topicBranch?: string; title?: string; description?: string; base?: string; mentions?: ResourceMention[]; attachments?: FileAttachment[]; profile?: string; agent?: string; model?: string; effort?: string; project?: { id: string | null; name: string }; hiddenBindingKeys?: string[] },
) {
  const draftUrl = composerUrl.value;
  const draftOwner = topicDraftOwners.get(draftUrl);
  const storage = { getItem: (key: string) => key === "loomUrl" ? draftUrl : localStorage.getItem(key),
    setItem: (key: string, value: string) => localStorage.setItem(key, value), removeItem: (key: string) => localStorage.removeItem(key) };
  let submitted: string | null = null;
  try { submitted = newTrackChat.value?.draftSnapshot() ?? storage.getItem(draftStorageKey(storage)); } catch { /* Storage unavailable. */ }
  return launchTask(task, repo, { ...meta, asTrack: true }, (success, currentComposer) => {
    if (success) {
      if (currentComposer) closeNewTopic();
      // Unmounting the sheet snapshots its draft (onUnmounted runs on
      // this same flush); drop it a microtask later, after that snapshot
      // has landed, so reopening the sheet starts fresh.
      nextTick(() => {
        if (topicDraftOwners.get(draftUrl) === draftOwner) clearSubmittedDraft(storage, submitted);
      });
    }
  });
}

// The thread sheet's launch: the sheet's draft carries attachments,
// resource mentions, and the launch config (the removed sidebar composer's
// affordances). Project preselection lives on the new-track chat — a
// project files tracks, so its + opens that chat.
function launchThread(
  task: string,
  repo: string,
  meta?: { mentions?: ResourceMention[]; attachments?: FileAttachment[]; profile?: string; agent?: string; model?: string; effort?: string },
) {
  return launchTask(task, repo, meta, (success, currentComposer) => {
    if (success && currentComposer) closeNewThreadSheet();
  });
}

// The durable command returns fresh tags. Reflect them across both panes immediately.
function onPolicyUpdated(view: SessionView) {
  for (const backend of backends.value) {
    backend.sessions = backend.sessions.map(session => session.id === view.id ? { ...session, branch: view.branch } : session);
  }
  if (selectedView.value?.id === view.id) selectedView.value = { ...selectedView.value, branch: view.branch };
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
    reportActionError(backendOf(id), e);
  }
}

async function delegateFromThread(parentId: string, task: string, completed?: (success: boolean) => void) {
  const launchUrl = backendOf(parentId);
  if (!launchUrl || pendingLaunches.value.has(launchUrl)) return;
  pendingLaunches.value.add(launchUrl);
  // The launch-preset picker's current selection rides along: the last
  // chosen harness/model/effort is the default for every delegation.
  const sel = launchSelectionFor(launchUrl);
  try {
    const view = await invoke<SessionView>("delegate_task", {
      parentId,
      task,
      agent: sel.agent || null,
      model: sel.model || null,
      effort: sel.effort || null,
    });
    completed?.(true);
    // Stay on the parent thread — the child appears nested under it in the
    // sidebar (and inherits the parent's topic) via the layout events.
  } catch (e: any) {
    reportActionError(launchUrl, e);
    completed?.(false);
  } finally {
    pendingLaunches.value.delete(launchUrl);
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
    reportActionError(backendOf(sessionId), e);
  }
}

async function deleteLane(laneId: string) {
  try {
    // Delete a user lane; its chats move back to the user-space Inbox.
    const inbox = userInboxId(backendOf(laneId));
    if (!inbox) {
      connError.value = "no Inbox lane found to move chats into";
      return;
    }
    await invoke("delete_group", {
      groupId: laneId,
      destinationGroupId: inbox,
    });
  } catch (e: any) {
    reportActionError(backendOf(laneId), e);
  }
}

function userInboxId(url: string | null): string | undefined {
  for (const sp of layout.value?.spaces ?? []) {
    if (backendOf(sp.id) !== url) continue;
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
    reportActionError(backendOf(id), e);
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

async function onUnarchived(id: string) {
  if (unarchivingIds.value.has(id)) return;
  const generation = connectionGeneration;
  unarchivingIds.value.add(id);
  try {
    const view = await invoke<SessionView>("unarchive_session", { id });
    if (generation !== connectionGeneration) return;
    // Keep the selected chat and sidebar usable even before the fleet event arrives.
    const summary = fleet.value.find((session) => session.id === id);
    if (summary) Object.assign(summary, view);
    if (selectedId.value === id && viewMode.value === "thread") await selectSession(id);
  } catch (e: any) {
    if (generation === connectionGeneration) connError.value = e?.message ?? String(e);
  } finally {
    unarchivingIds.value.delete(id);
  }
}

const connClass = computed(() => backends.value.every(backend => backend.available) ? "ok" : connected.value ? "warn" : "bad");

// Cached summaries belong to one server, even if another server reuses an ID.
const inspectorServer = computed(() => backendOf(selectedId.value) || backendOf(selectedTopicId.value) || loomUrl.value);
watch(inspectorServer, () => { selectedSummarySnapshot.value = null; }, { flush: "sync" });
// Refresh known ancestry when the first summary arrives, and retain it across gaps.
watch(() => fleet.value.find(session => session.id === selectedId.value), summary => {
  if (summary) selectedSummarySnapshot.value = summary;
});
// Retain the latest summary if its fleet row is delayed. View-only fields stay
// in selectedView; the initial view must never overwrite newer live metadata.
const inspectorFleet = computed<SessionSummary[]>(() => {
  const view = selectedView.value;
  const summary = selectedSummarySnapshot.value;
  return view && summary?.id === view.id && !fleet.value.some(session => session.id === view.id)
    ? [...fleet.value, summary] : fleet.value;
});
const selectedTopic = computed(() => {
  let node = inspectorFleet.value.find((session) => session.id === (selectedId.value ?? selectedTopicId.value));
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
  return node && !isStandalone(node) && isTrackCoordinator(node) ? node : null;
});
// Keep component identity across temporary ancestor gaps, separately from live
// scope metadata. Missing ancestors must still disable Track policy/actions.
const inspectorRoute = computed(() => JSON.stringify([inspectorServer.value, selectedId.value, selectedTopicId.value]));
const inspectorAnchor = ref<{ route: string; id: string; server: string } | null>(null);
// Ancestry enrichment changes scope, never the identity of an open inspector.
// Verified navigation within one Track keeps its tab; ChangeReview resets its
// source state on sessionId. Other navigation/connection changes reset the panel.
const inspectorInstanceKey = ref(inspectorRoute.value);
watch([inspectorRoute, selectedTopic], ([route, topic], [previousRoute]) => {
  const sameTrack = topic && inspectorAnchor.value?.id === topic.id
    && inspectorAnchor.value.server === inspectorServer.value;
  if (route !== previousRoute && !sameTrack) inspectorInstanceKey.value = route;
  if (topic) inspectorAnchor.value = { route, id: topic.id, server: inspectorServer.value };
  else if (inspectorAnchor.value?.route !== route) inspectorAnchor.value = null;
}, { immediate: true, flush: "sync" });
// Standalone chats can open Resources and Review without becoming Tracks.
const inspectorTopic = computed(() => selectedTopic.value ?? ((inspectorAnchor.value || artifactRequest.value || reviewRequest.value)
  ? inspectorFleet.value.find(session => session.id === selectedId.value) ?? selectedView.value : null));
watch([inspectorServer, selectedId, selectedTopicId], () => { artifactRequest.value = null; reviewRequest.value = null; });
const showProjectSheet = computed(() => showNewProject.value || !!projectResourcesProject.value);
const inspectorVisible = computed(() => !showNewThread.value && !showNewTopic.value && !showProjectSheet.value
  && viewMode.value !== "home" && !!inspectorTopic.value && showResources.value && currentAvailable.value);
// Closing or leaving the inspector always restores the conversation layout.
watch(inspectorVisible, visible => { if (!visible) inspectorExpanded.value = false; });
const expandedInspectorVisible = computed(() => inspectorVisible.value && inspectorExpanded.value
  && viewMode.value === "thread" && !!selectedView.value);
</script>

<template>
  <div class="app" :class="{ 'with-resources': inspectorVisible, 'inspector-expanded': expandedInspectorVisible }" data-tauri-drag-region>
    <header class="header" data-tauri-drag-region>
      <button class="title home-link" title="Show Tracks home" @click="onHome">🕸 Arachne</button>
      <button class="header-search" title="Search and commands (⌘K)" @click="showSearch = true">Search <kbd>⌘K</kbd></button>
      <button
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
              : `${backends.filter(backend => backend.available).length}/${backends.length} servers available`
            : (connError ?? (backends.some(backend => backend.connecting) ? "connecting…" : "Backends unavailable"))
        }}
      </button>
      <button v-if="!showProjectSheet && viewMode !== 'home' && selectedTopic" class="header-resources" :aria-pressed="showResources"
        title="Track inspector (⌘⇧E)" @click="showResources = !showResources">Inspector</button>
    </header>
    <FleetSidebar
      :fleet="fleet"
      :layout="layout"
      :backends="backends"
      @retry-backend="retryBackend"
      :selected-id="selectedId ?? selectedTopicId"
      :show-new-thread="showNewThread"
      :show-new-topic="showNewTopic"
      :show-new-project="showNewProject"
      @select="selectSession"
      @select-topic="selectTopic"
      @update-topic="updateTopic"
      @new-thread="openNewThread"
      @new-topic="openNewTopic"
      @new-project="openNewProject"
      @new-topic-in-project="openNewTopicInProject"
      @manage-project-resources="openProjectResources"
      @select-project="selectProject"
      @reparent="reparentSession"
      @delete-lane="deleteLane"
      @archive="onArchived"
      :unarchiving-ids="unarchivingIds"
      @unarchive="onUnarchived"
    />
    <NewTrackChat
      ref="newTrackChat"
      v-if="showNewTopic"
      :key="`${composerUrl}:${composerGeneration}`"
      :backend-url="composerUrl"
      :backends="backends"
      :available="currentAvailable"
      @backend="changeComposerBackend"
      :fleet="composerFleet"
      :launching="launching"
      :error="connError"
      :launch-options="launchOptions"
      :layout="composerLayout"
      :project="newTopicProject"
      @close="closeNewTopic"
      @launch="launchTopic"
    />
    <ProjectSheet
      v-else-if="showProjectSheet"
      :key="projectResourcesProject?.id ?? composerUrl"
      :project="projectResourcesProject"
      :layout="composerLayout"
      :backend-url="operationUrl"
      :backends="backends"
      :available="currentAvailable"
      @backend="changeComposerBackend"
      @close="closeProjectResources"
      @created="onProjectCreated"
      @layout="onProjectLayout"
    />
    <NewThreadSheet
      v-else-if="showNewThread"
      :key="`${composerUrl}:${composerGeneration}`"
      :available="currentAvailable"
      :fleet="composerFleet"
      :launching="launching"
      :error="connError"
      :launch-options="launchOptions"
      @close="closeNewThreadSheet"
      @launch="launchThread"
    />
    <ThreadView
      ref="threadView"
      v-else-if="currentAvailable && viewMode === 'thread' && selectedId && selectedView"
      :key="selectedId"
      :inspector-expanded="expandedInspectorVisible"
      @review="openReview"
      :session="selectedView"
      :topic="selectedTopic"
      :fleet="inspectorFleet"
      :launch-options="launchOptions"
      :loom-url="backendOf(selectedView.id) || operationUrl"
      @open-artifact="openArtifact"
      @error="connError = $event"
      @archive="onArchived"
      @open-changes="showResources = true; landingRequest++"
      :unarchiving="unarchivingIds.has(selectedId)"
      @unarchive="onUnarchived"
      @delegate="delegateFromThread"
      @handoff="selectSession"
      @refresh="selectSession"
      @policy-updated="onPolicyUpdated"
      @open-topic="selectTopic"
      @overview="showTopicOverview"
      @open-coordinator="openCoordinatorThread"
      @home="onHome"
    />
    <section v-else-if="(viewMode !== 'home' || !!selectedProject?.id) && !currentAvailable" class="backend-unavailable main-unavailable" role="status">
      <h2>Backend unavailable</h2><p>{{ currentBackend?.name }} · {{ operationUrl }}</p>
      <p>{{ currentBackend?.error || 'Reconnecting…' }}</p>
      <button :disabled="currentBackend?.connecting" @click="retryBackend(operationUrl)">{{ currentBackend?.connecting ? 'Connecting…' : 'Reconnect' }}</button>
    </section>
    <HomeView
      v-else
      :key="`${viewMode === 'topic' ? selectedTopicId ?? 'topic' : 'home'}`"
      :fleet="fleet"
      :topic="viewMode === 'topic' ? selectedTopic : null"
      :project="selectedProject"
      :layout="layout"
      @select="selectSession"
      @new-thread="openNewThread"
      @new-topic="openNewTopic"
      @open-zed="openTopicInZed"
      @home="onHome"
      @manage-project-resources="openProjectResources"
      @error="connError = $event"
    />
    <TopicInspector v-if="inspectorVisible && inspectorTopic" :key="inspectorInstanceKey"
      :scope-id="inspectorAnchor?.id ?? inspectorTopic.id"
      :expanded="expandedInspectorVisible" :can-expand="viewMode === 'thread' && !!selectedView"
      :review-request="reviewRequest" @toggle-expanded="inspectorExpanded = !inspectorExpanded" :topic="inspectorTopic" :selected-view="selectedView" :fleet="inspectorFleet" :selected-id="selectedId" :loom-url="inspectorServer"
      :landing-request="landingRequest"
      :artifact-request="artifactRequest" @open-artifact="openArtifact"
      @close="showResources = false" @error="connError = $event" @select="selectInspectorSession" @new-thread="openWorker" @checkout-saved="threadView?.refreshWorkSummary($event)" @policy-updated="onPolicyUpdated" />
    <AttentionBanner v-show="connected && !showSettings && !showNewTopic && !showNewThread && !showProjectSheet && viewMode !== 'home'" :fleet="fleet" :selected-id="selectedId" :connection-key="loomUrl" @select="selectSession" @home="onHome" />
    <CommandPalette v-if="showSearch" :fleet="fleet" @close="showSearch = false" @select="selectSession" @action="command" />
    <div v-if="connError && !showSettings && !showNewThread && !showNewTopic" class="app-error" role="alert"><span>{{ connError }}</span><button @click="showSettings = true" v-if="!connected">Connection settings</button><button aria-label="Dismiss error" @click="connError = null">×</button></div>
    <SettingsSheet
      v-if="showSettings"
      :url="loomUrl"
      :token="loomToken"
      :backends="backends"
      :connected="connected"
      :error="settingsError"
      :saving="settingsSaving"
      @close="showSettings = false"
      @save="saveSettings"
    />
  </div>
</template>

<style scoped>
.main-unavailable { grid-area: main; padding: 36px; color: var(--muted); }
</style>
