<script setup lang="ts">
import { ref, computed, onMounted, onUnmounted, nextTick, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-shell";
import type { LaunchOptions, SessionSummary, SessionView } from "../App.vue";
import ContextTelemetry from "./ContextTelemetry.vue";
import { useSlashCommands, type SlashCommand } from "../useSlashCommands";
import LaunchPreset from "./LaunchPreset.vue";
import SplitButton from "./SplitButton.vue";
import CodeActions from "./CodeActions.vue";
import ChangeReview from "./ChangeReview.vue";
import { reviewIntegrationTarget } from "../reviewRequest";
import { addAttachments, filesFromClipboard, imagePreviewUrl, type FileAttachment } from "../attachments";
import { pasteAsPlainText } from "../composerPaste";
import ChatMarkdown from "./ChatMarkdown.vue";
import ChatImages from "./ChatImages.vue";
import CopyButton from "./CopyButton.vue";
import { groupDisplayBlocks, formatTokens, blockCopyText, toolCallCopyText, copyCornerFor, resolveContextUsage, commandReplyTurns, type ChatDisplayBlock } from "../chatRows";
import { useFileCompletion } from "../useFileCompletion";
import { markdownForSelection } from "../markdownCopy";
import { bodyOffset } from "../selectionOffsets";
import { isStandalone } from "../threadKind";
import { dismissibleAttention, composerSendAsDismiss } from "../topicInspector";

interface Cursor {
  turn: number;
  seq: number;
}

// fetch_chat's reply: the journal page plus the live-turn signals behind
// the working indicator. `live_turn` is set while an ACP turn is in
// flight; `pending_prompt` is a message queued behind it, waiting to start
// its own turn; the two timestamps restore the elapsed clock after a
// reload (the turn's opening message = start, newest block = progress).
interface ChatSnapshot {
  blocks: ChatDisplayBlock[];
  metadata: AcpMetadata;
  live_turn: number | null;
  pending_prompt: string | null;
  live_started_at: string | null;
  live_progress_at: string | null;
}

interface ConfigChoice { value: string; name: string; description?: string | null }
interface ConfigOption {
  id: string;
  name: string;
  category?: string | null;
  type: string;
  currentValue: string | boolean;
  options?: (ConfigChoice | { group: string; name: string; options: ConfigChoice[] })[];
}
interface AcpMetadata { config_options: ConfigOption[]; commands: SlashCommand[]; modes: unknown[]; steering_supported: boolean }

// One `loom://chat-event` frame: {topic, event, data}. On the chat topic
// loom emits turn / block / delta / tool / queue (and resync when a
// bounded broadcast drops frames); on the session topic, status/tag kinds.
interface ChatEventFrame {
  topic: string;
  event: string;
  data?: any;
}

const props = defineProps<{ session: SessionView; topic: SessionSummary | null; fleet: SessionSummary[]; launchOptions: LaunchOptions | null; loomUrl: string }>();
const currentUsage = computed(() => {
  const summary = props.fleet.find((item) => item.id === props.session.id);
  return resolveContextUsage(blocks.value, summary?.usage ?? props.session.usage);
});
const emit = defineEmits<{
  (e: "error", msg: string): void;
  (e: "archive", id: string): void;
  (e: "delegate", parentId: string, task: string, completed: (success: boolean) => void): void;
  (e: "handoff", id: string): void;
  (e: "refresh", id: string): void;
  (e: "open-topic", id: string): void;
  (e: "overview", id: string): void;
  (e: "open-coordinator", id: string): void;
  (e: "home"): void;
}>();

// Dismiss this thread's attention flag: clear the loud tag axes on its
// branch. Same clear_attention command as the home and sidebar rows —
// here it answers the flag directly, at the conversation it came from.
// The button's visibility tracks the LIVE fleet summary (tags update over
// SSE), not the once-fetched SessionView — otherwise the button would
// linger after a successful dismiss.
const liveSummary = computed(
  () => props.fleet.find((s) => s.id === props.session.id) ?? props.session,
);
const canDismiss = computed(() => dismissibleAttention(liveSummary.value));
const dismissing = ref(false);
async function dismissAttention() {
  if (dismissing.value) return;
  dismissing.value = true;
  try {
    await invoke("clear_attention", { session: props.session.id });
  } catch (error: any) {
    emit("error", error?.message ?? String(error));
  } finally {
    dismissing.value = false;
  }
}

const blocks = ref<ChatDisplayBlock[]>([]);
const metadata = ref<AcpMetadata | null>(null);
const configBusy = ref<string | null>(null);
const modelConfig = computed(() => metadata.value?.config_options.find((option) => option.type === "select" && (option.category === "model" || option.id === "model")));
const effortConfig = computed(() => metadata.value?.config_options.find((option) => option.type === "select" && (option.category === "thought_level" || option.id === "thought_level" || option.id === "reasoning_effort")));
function configChoices(option: ConfigOption): ConfigChoice[] {
  return (option.options ?? []).flatMap((choice) => "group" in choice ? choice.options : [choice]);
}
async function setConfig(option: ConfigOption, event: Event) {
  const value = (event.target as HTMLSelectElement).value;
  if (value === option.currentValue || configBusy.value) return;
  const id = props.session.id;
  configBusy.value = option.id;
  try {
    const updated = await invoke<AcpMetadata>("set_session_config", { id, configId: option.id, value });
    if (props.session.id === id) metadata.value = updated;
  } catch (error: any) {
    (event.target as HTMLSelectElement).value = String(option.currentValue);
    emit("error", error?.message ?? String(error));
  } finally {
    configBusy.value = null;
  }
}
const rows = computed(() => groupDisplayBlocks(
  blocks.value,
  turnLive.value ? snapshotLiveTurn.value : null,
  props.session.agent_kind === "pi",
));
// Turn number → the command pi-acp's interceptor answered locally. Those
// turns' replies are system output (stats, receipts), not agent prose.
const commandTurns = computed(() => commandReplyTurns(blocks.value, metadata.value?.commands.map((c) => c.name) ?? []));
/** The block is an interceptor reply (not the user's command, not prose). */
const isCommandReply = (block: ChatDisplayBlock) =>
  block.kind === "agent_message" && commandTurns.value.has(block.turn ?? -1);
const answeringPermission = ref<string | null>(null);
const permissionError = ref<Record<string, string>>({});

async function answerPermission(block: ChatDisplayBlock, optionId: string) {
  const requestId = block.request_id;
  if (!requestId || answeringPermission.value) return;
  answeringPermission.value = requestId;
  permissionError.value = { ...permissionError.value, [requestId]: "" };
  try {
    await invoke("answer_permission", { id: props.session.id, requestId, optionId });
    scheduleReload();
  } catch (error: any) {
    permissionError.value = { ...permissionError.value, [requestId]: error?.message ?? String(error) };
    scheduleReload();
  } finally {
    answeringPermission.value = null;
  }
}

const draft = ref("");
const completion = useFileCompletion(draft, computed(() => props.session.id));
const attachments = ref<FileAttachment[]>([]);
const attachmentError = ref("");
const attachmentLoading = ref(false);
async function addFiles(files: FileList | File[]) {
  if (attachmentLoading.value) return;
  attachmentLoading.value = true;
  try { attachments.value = await addAttachments(attachments.value, files); attachmentError.value = ""; }
  catch (error: any) { attachmentError.value = error?.message ?? String(error); }
  finally { attachmentLoading.value = false; }
}
function onFileInput(event: Event) {
  const input = event.target as HTMLInputElement;
  if (input.files) void addFiles(input.files);
  input.value = "";
}
function onComposerPaste(event: ClipboardEvent) {
  if (!event.clipboardData) return;
  // A turn copied from the conversation carries its raw markdown as
  // text/plain and the rendered markup as text/html; the composer is a
  // plain-text editor, so take the markdown flavor before WebKit
  // flattens the HTML one into the draft.
  if (pasteAsPlainText(event)) {
    event.preventDefault();
    const text = event.clipboardData.getData("text/plain");
    document.execCommand("insertText", false, text);
    completion.updateCaret();
    return;
  }
  const files = filesFromClipboard(event.clipboardData);
  if (!files.length) return;
  event.preventDefault(); void addFiles(files);
}
interface MentionResource {
  id: string;
  kind: string;
  title: string;
  path: string | null;
  url: string | null;
  reference: string | null;
  /** Inherited rows the track hid — never offerable as a mention. */
  hidden?: boolean;
}
const composerEl = ref<HTMLTextAreaElement | null>(null);
const slash = useSlashCommands(draft, computed(() => metadata.value?.commands ?? []), composerEl);
const canCompact = computed(() => metadata.value?.commands.some((command) => command.name === "compact") ?? false);
const mentionResources = ref<MentionResource[]>([]);
const mentionLoading = ref(false);
const mentionError = ref("");
const mentionRange = ref<{ start: number; end: number; query: string } | null>(null);
const mentionIndex = ref(0);
const selectedMentions = ref<{ id: string; token: string }[]>([]);
const matchingResources = computed(() => {
  const query = mentionRange.value?.query.trim().toLowerCase() ?? "";
  return mentionResources.value.filter((resource) =>
    !query || [resource.title, resource.kind, resource.path, resource.url, resource.reference]
      .some((value) => value?.toLowerCase().includes(query)),
  ).slice(0, 8);
});

async function loadMentionResources() {
  const topicId = props.topic?.id;
  if (!topicId) return;
  mentionLoading.value = true;
  mentionError.value = "";
  try {
    const view = await invoke<{ resources: MentionResource[] }>("topic_resources", { topicId });
    // Effective rows only: hidden inherited bindings never resolve.
    if (props.topic?.id === topicId) mentionResources.value = (view.resources ?? []).filter((resource) => !resource.hidden);
  } catch (error: any) {
    if (props.topic?.id === topicId) mentionError.value = error?.message ?? String(error);
  } finally {
    mentionLoading.value = false;
  }
}

function updateMention() {
  const caret = composerEl.value?.selectionStart ?? draft.value.length;
  const before = draft.value.slice(0, caret);
  const match = /(?:^|\s)@([^@{}\n]{0,64})$/.exec(before);
  const wasOpen = !!mentionRange.value;
  mentionRange.value = match ? { start: caret - match[1].length - 1, end: caret, query: match[1] } : null;
  mentionIndex.value = 0;
  if (mentionRange.value && !wasOpen) void loadMentionResources();
}

function chooseMention(resource: MentionResource) {
  const range = mentionRange.value;
  if (!range) return;
  const duplicateTitle = mentionResources.value.some((other) => other.id !== resource.id && other.title === resource.title);
  const label = duplicateTitle ? `${resource.title} (${resource.path || resource.url || resource.reference || resource.kind})` : resource.title;
  const token = `@{${label}}`;
  draft.value = draft.value.slice(0, range.start) + token + " " + draft.value.slice(range.end);
  selectedMentions.value.push({ id: resource.id, token });
  mentionRange.value = null;
  nextTick(() => {
    const caret = range.start + token.length + 1;
    composerEl.value?.focus();
    composerEl.value?.setSelectionRange(caret, caret);
  });
}

function onComposerKeydown(event: KeyboardEvent) {
  if (slash.onKeydown(event)) return;
  if (completion.onKeydown(event)) return;
  if (event.key === "Enter" && !event.shiftKey && (event.metaKey || event.ctrlKey)) {
    event.preventDefault();
    if (mentionRange.value && matchingResources.value.length) chooseMention(matchingResources.value[mentionIndex.value] || matchingResources.value[0]);
    // Empty composer + idle agent + flagged thread: the send button is the
    // dismiss button, so ⌘/Ctrl+Enter affirms the same way.
    else if (sendAsDismiss.value) void dismissAttention();
    else void send();
    return;
  }
  if (!mentionRange.value) return;
  if (event.key === "Escape") { event.preventDefault(); mentionRange.value = null; }
  else if (event.key === "ArrowDown" && matchingResources.value.length) {
    event.preventDefault(); mentionIndex.value = (mentionIndex.value + 1) % matchingResources.value.length;
  } else if (event.key === "ArrowUp" && matchingResources.value.length) {
    event.preventDefault(); mentionIndex.value = (mentionIndex.value - 1 + matchingResources.value.length) % matchingResources.value.length;
  }
}

watch(() => props.topic?.id, () => {
  mentionResources.value = [];
  mentionRange.value = null;
  selectedMentions.value = [];
});
const busy = ref(false);
const loadingOlder = ref(false);
const hasOlder = ref(true);
const convEl = ref<HTMLElement | null>(null);
const unlisteners: UnlistenFn[] = [];

// --- Live-turn state (the working spinner) ---------------------------------
//
// A turn is live from the `turn started` SSE frame (or a snapshot with
// `live_turn` set — it's durable, so it survives an app reload) until the
// `turn ended` frame. `clock` ticks once a second to keep the elapsed
// label moving; `sseTurnAt` remembers when the last turn event arrived so a
// reload that raced a turn boundary never clobbers fresher SSE truth.
const turnLive = ref(false);
// The live turn's journal coordinate: a work group whose members belong to
// it keeps rendering its last thought live (streamed thinking shows no
// journal block until it closes).
const snapshotLiveTurn = ref<number | null>(null);
const turnStartedAt = ref<number | null>(null);
const lastProgressAt = ref<number | null>(null);
const pendingPrompt = ref<string | null>(null);
const clock = ref(Date.now());
let ticker: ReturnType<typeof setInterval> | null = null;
let sseTurnAt = 0;

function markProgress(at?: number) {
  const t = at ?? Date.now();
  lastProgressAt.value = Math.max(lastProgressAt.value ?? 0, t);
  if (turnStartedAt.value == null) turnStartedAt.value = t;
}

function turnStarted() {
  turnLive.value = true;
  snapshotLiveTurn.value = null;
  turnStartedAt.value = Date.now();
  lastProgressAt.value = Date.now();
  pendingPrompt.value = null;
  sseTurnAt = Date.now();
  if (atBottom()) scrollToBottom();
}

function turnEnded() {
  turnLive.value = false;
  snapshotLiveTurn.value = null;
  turnStartedAt.value = null;
  lastProgressAt.value = null;
  sseTurnAt = Date.now();
}

// Apply a snapshot's live-turn signals. A snapshot fetched before a turn
// boundary (mid-flight during `send`, or racing an SSE turn event) must not
// downgrade liveness an event newer than the fetch already established —
// compare `sseTurnAt` against when the fetch began.
function applyLive(snap: ChatSnapshot, fetchedAt: number) {
  pendingPrompt.value = (snap.pending_prompt ?? "").trim() || null;
  const sseIsNewer = sseTurnAt >= fetchedAt;
  if (snap.live_turn != null) {
    turnLive.value = true;
    snapshotLiveTurn.value = snap.live_turn;
    // Restore the elapsed clock from the journal, never regressing what
    // streaming already observed (mirrors loom SPA's preserve semantics).
    if (snap.live_started_at) {
      const restored = Date.parse(snap.live_started_at);
      if (!Number.isNaN(restored)) {
        turnStartedAt.value = Math.min(turnStartedAt.value ?? Infinity, restored);
      }
    } else if (turnStartedAt.value == null) {
      turnStartedAt.value = Date.now();
    }
    if (snap.live_progress_at) {
      const restored = Date.parse(snap.live_progress_at);
      if (!Number.isNaN(restored)) markProgress(restored);
    }
  } else if (!sseIsNewer) {
    turnLive.value = false;
    snapshotLiveTurn.value = null;
    turnStartedAt.value = null;
    lastProgressAt.value = null;
  }
}

function onChatFrame(frame: ChatEventFrame) {
  const d = frame.data ?? {};
  switch (frame.event) {
    case "turn":
      if (d.turn != null) snapshotLiveTurn.value = d.turn;
      if (d.state === "started") turnStarted();
      else turnEnded();
      break;
    case "delta":
    case "tool":
      // Only stream mid-turn; they are proof of life.
      turnLive.value = true;
      markProgress();
      break;
    case "block":
      if (turnLive.value) markProgress();
      break;
    case "queue":
      pendingPrompt.value = (d.pending_prompt ?? "").trim() || null;
      break;
    case "metadata":
      metadata.value = d as AcpMetadata;
      break;
  }
  scheduleReload();
}

const elapsedLabel = computed(() => {
  if (turnStartedAt.value == null) return "";
  const s = Math.max(0, Math.floor((clock.value - turnStartedAt.value) / 1000));
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
});
const progressAge = computed(() =>
  lastProgressAt.value == null
    ? 0
    : Math.max(0, Math.floor((clock.value - lastProgressAt.value) / 1000)),
);

// Delegation: spawn a child session under this one. The child lands in the
// same topic (loom inherits the parent's placement group) and nests
// under this row in the sidebar.
const showDelegate = ref(false);
const delegateTask = ref("");
const delegateInput = ref<HTMLInputElement | null>(null);
async function openDelegate() { showDelegate.value = true; await nextTick(); delegateInput.value?.focus(); }
defineExpose({ openDelegate, refreshWorkSummary });
const delegating = ref(false);

const showSendToThread = ref(false);
const sendDestination = ref("");
const sendNote = ref("");
const sendingToThread = ref(false);
const deliveryKey = ref(crypto.randomUUID());
const destinations = computed(() =>
  props.fleet
    .filter((s) => s.id !== props.session.id && s.status !== "archived")
    .sort((a, b) => b.last_activity_at.localeCompare(a.last_activity_at)),
);

const showHandoff = ref(false);
const handoffProfile = ref(props.session.profile || "default");
const handoffAgent = ref(props.session.agent_kind || "");
const handoffModel = ref(props.session.model || "");
const handoffEffort = ref(props.session.effort || "");
const handingOff = ref(false);
const currentStatus = computed(() =>
  props.fleet.find((s) => s.id === props.session.id)?.status ?? props.session.status,
);
const canSend = computed(() => currentStatus.value === "running" || currentStatus.value === "orphaned");
// The composer's send button doubles as the dismiss button when the
// thread is flagged, the agent is resting, and nothing is queued: the
// yellow attention circle with a checkmark instead of the send arrow.
// Typing (or attaching) reverts it to Send, so the affirmation never
// hides the send path. "Working" is cross-protocol: turnLive for ACP,
// and for terminal sessions the absence of loom's quiet `idle` mark
// while running (the same rest/alive distinction the sidebar rows use).
const idleMark = computed(() =>
  liveSummary.value.branch.tags.some((tag) => tag.key === "idle"),
);
const sendAsDismiss = computed(() =>
  composerSendAsDismiss({
    dismissible: canDismiss.value,
    working: turnLive.value || busy.value || pendingPrompt.value != null || attachmentLoading.value ||
      (props.session.protocol !== "acp" && currentStatus.value === "running" && !idleMark.value),
    hasContent: !!draft.value.trim() || attachments.value.length > 0,
  }),
);
const canInterrupt = computed(() =>
  currentStatus.value === "running" && (props.session.protocol !== "acp" || turnLive.value),
);
const profileAgent = computed(() =>
  props.launchOptions?.profiles.find((p) => p.name === handoffProfile.value)?.agent_kind ||
  props.launchOptions?.default_agent || "",
);
const handoffAgentChoice = computed(() =>
  props.launchOptions?.agents.find((a) => a.kind === (handoffAgent.value || profileAgent.value)),
);
const handoffAllowed = computed(() => {
  const current = props.fleet.find((s) => s.id === props.session.id);
  return props.session.protocol === "acp" &&
    current?.status === "running" &&
    current.branch.tags.some((tag) => tag.key === "idle") && !turnLive.value;
});

function chooseHandoffProfile() {
  // A new profile supplies its own agent/model/effort defaults.
  handoffAgent.value = "";
  handoffModel.value = "";
  handoffEffort.value = "";
}

function chooseHandoffAgent() {
  handoffModel.value = "";
  handoffEffort.value = "";
}

async function handoff() {
  if (!handoffAllowed.value || handingOff.value) return;
  handingOff.value = true;
  try {
    await invoke("handoff_session", {
      id: props.session.id,
      profile: handoffProfile.value || "default",
      agent: handoffAgent.value || null,
      model: handoffModel.value.trim() || null,
      effort: handoffEffort.value || null,
    });
    showHandoff.value = false;
    emit("handoff", props.session.id);
  } catch (e: any) {
    emit("error", e?.message ?? String(e));
  } finally {
    handingOff.value = false;
  }
}

watch([sendDestination, sendNote], () => {
  deliveryKey.value = crypto.randomUUID();
});

async function sendThreadNote() {
  const note = sendNote.value.trim();
  if (!note || !sendDestination.value || sendingToThread.value) return;
  sendingToThread.value = true;
  try {
    await invoke("send_to_thread", {
      sourceId: props.session.id,
      destinationId: sendDestination.value,
      note,
      idempotencyKey: deliveryKey.value,
    });
    sendNote.value = "";
    sendDestination.value = "";
    showSendToThread.value = false;
  } catch (e: any) {
    emit("error", e?.message ?? String(e));
  } finally {
    sendingToThread.value = false;
  }
}

function submitDelegate() {
  const t = delegateTask.value.trim();
  if (!t || delegating.value) return;
  delegating.value = true;
  emit("delegate", props.session.id, t, success => {
    delegating.value = false;
    if (success) { delegateTask.value = ""; showDelegate.value = false; }
  });
}

// Auto-scroll only when the user is already at (or near) the bottom — never
// yank someone who has scrolled up to read.
function atBottom(): boolean {
  if (!convEl.value) return true;
  const el = convEl.value;
  return el.scrollHeight - el.scrollTop - el.clientHeight < 60;
}
function scrollToBottom() {
  nextTick(() => {
    if (convEl.value) convEl.value.scrollTop = convEl.value.scrollHeight;
  });
}

// Debounced reload: SSE events arrive in bursts (one turn journals many
// blocks); re-fetching per event hammers loom and re-renders constantly.
let reloadTimer: ReturnType<typeof setTimeout> | null = null;
function scheduleReload() {
  if (reloadTimer) return;
  reloadTimer = setTimeout(async () => {
    reloadTimer = null;
    await reload();
  }, 300);
}

async function reload() {
  const stick = atBottom();
  const fetchedAt = Date.now();
  try {
    const snap = await invoke<ChatSnapshot>("fetch_chat", {
      id: props.session.id,
    });
    blocks.value = snap.blocks;
    metadata.value = snap.metadata;
    if (snap.live_turn != null) snapshotLiveTurn.value = snap.live_turn;
    else if (!turnLive.value) snapshotLiveTurn.value = null;
    applyLive(snap, fetchedAt);
    const cursor = await invoke<Cursor | null>("chat_older_cursor");
    hasOlder.value = cursor !== null;
    if (stick) scrollToBottom();
  } catch (e: any) {
    emit("error", e?.message ?? String(e));
  }
}

// Prepend the previous page of history, keeping the scroll anchored to
// where the user is (loading older shouldn't jump the viewport).
async function loadOlder() {
  if (loadingOlder.value || !hasOlder.value) return;
  const cursor = await invoke<Cursor | null>("chat_older_cursor");
  if (!cursor) {
    hasOlder.value = false;
    return;
  }
  loadingOlder.value = true;
  try {
    // An older page carries its own (stale-tail) liveness fields; only the
    // blocks are wanted here — applyLive runs on newest-tail reloads only.
    const older = (
      await invoke<ChatSnapshot>("fetch_chat", {
        id: props.session.id,
        beforeTurn: cursor.turn,
        beforeSeq: cursor.seq,
      })
    ).blocks;
    if (older.length === 0) {
      hasOlder.value = false;
    } else {
      const el = convEl.value;
      const beforeHeight = el?.scrollHeight ?? 0;
      blocks.value = [...older, ...blocks.value];
      await nextTick();
      if (el) el.scrollTop += el.scrollHeight - beforeHeight;
    }
    const next = await invoke<Cursor | null>("chat_older_cursor");
    hasOlder.value = next !== null;
  } catch (e: any) {
    emit("error", e?.message ?? String(e));
  } finally {
    loadingOlder.value = false;
  }
}

// Journal coordinates keep disclosure state stable when older pages prepend.
const collapsed = ref<Record<string, boolean>>({});

// --- Copy-icon corner following -------------------------------------------
//
// A block's copy icon pins to the top-right corner of its bubble while
// that corner is on screen, and flips to the bottom-right once the top
// has scrolled out of view. The corner depends on pure geometry (block
// top vs scroll container top), recomputed on scroll (rAF-coalesced) and
// whenever rows repaint. Host elements carry the row key on
// data-copy-host so the handler can walk the rendered rows.
const copyCorners = ref<Record<string, "top" | "bottom">>({});
let cornerRaf: number | null = null;

function recomputeCorners() {
  const scroller = convEl.value;
  if (!scroller) return;
  const viewportTop = scroller.getBoundingClientRect().top;
  const next: Record<string, "top" | "bottom"> = {};
  for (const el of Array.from(scroller.querySelectorAll<HTMLElement>("[data-copy-host]"))) {
    next[el.dataset.copyHost!] = copyCornerFor(el.getBoundingClientRect().top, viewportTop);
  }
  copyCorners.value = next;
}

function onConversationScroll() {
  if (cornerRaf != null) return;
  cornerRaf = requestAnimationFrame(() => {
    cornerRaf = null;
    recomputeCorners();
  });
}

function cornerOf(key: string): "top" | "bottom" {
  return copyCorners.value[key] ?? "top";
}

// Rows repaint after every fetch or page load; disclosure toggles mutate
// `collapsed` in place and change every host's height. Both recomputations
// wait a tick for the DOM to settle. (Deep watch on `collapsed` only:
// deep-watching `rows` would traverse every block payload each reload.)
watch(rows, () => {
  nextTick(recomputeCorners);
});
watch(collapsed, () => {
  nextTick(recomputeCorners);
}, { deep: true });

onMounted(async () => {
  ticker = setInterval(() => (clock.value = Date.now()), 1000);
  await reload();
  scrollToBottom();
  recomputeCorners();
  unlisteners.push(
    await listen("loom://chat-event", (event) => {
      const frame = event.payload as ChatEventFrame;
      if (frame.topic === "connection" && frame.event === "resync") { scheduleReload(); return; }
      if (
        frame.topic.startsWith("chat:") ||
        frame.topic.startsWith("session:")
      ) {
        onChatFrame(frame);
      }
    }),
  );
});

// Defense in depth: the parent keys this component by session id so a
// different session should always remount, but if the id ever changes under
// a mounted instance (state desync, future refactor), refetch rather than
// show the wrong thread.
watch(
  () => props.session.id,
  async (newId, oldId) => {
    if (newId !== oldId) {
      collapsed.value = {};
      hasOlder.value = true;
      blocks.value = [];
      turnLive.value = false;
      turnStartedAt.value = null;
      lastProgressAt.value = null;
      pendingPrompt.value = null;
      snapshotLiveTurn.value = null;
      sseTurnAt = 0;
      await reload();
      scrollToBottom();
    }
  },
);

onUnmounted(() => {
  unlisteners.forEach((u) => u());
  if (reloadTimer) clearTimeout(reloadTimer);
  if (ticker) clearInterval(ticker);
  if (cornerRaf != null) cancelAnimationFrame(cornerRaf);
});

async function send() {
  if (mentionRange.value && matchingResources.value.length) {
    chooseMention(matchingResources.value[mentionIndex.value] || matchingResources.value[0]);
  }
  const text = draft.value.trim();
  if (!canSend.value || (!text && !attachments.value.length) || busy.value || attachmentLoading.value) return;
  const wasOrphaned = currentStatus.value === "orphaned";
  busy.value = true;
  try {
    await invoke("send_input", {
      id: props.session.id,
      text: text || "Please inspect the attached files.",
      topicId: props.topic?.id ?? null,
      resourceIds: selectedMentions.value.filter((mention) => text.includes(mention.token)).map((mention) => mention.id),
      attachments: attachments.value.map(({ name, contentBase64 }) => ({ name, contentBase64 })),
    });
    draft.value = "";
    attachments.value = [];
    selectedMentions.value = [];
    mentionRange.value = null;
    await reload();
    if (wasOrphaned) emit("refresh", props.session.id);
  } catch (e: any) {
    emit("error", e?.message ?? String(e));
  } finally {
    busy.value = false;
  }
}

async function compactContext() {
  if (!canCompact.value || !canSend.value || turnLive.value || busy.value) return;
  busy.value = true;
  try {
    await invoke("send_input", {
      id: props.session.id,
      text: "/compact",
      topicId: props.topic?.id ?? null,
      resourceIds: [],
      attachments: [],
    });
    await reload();
  } catch (error: any) {
    emit("error", error?.message ?? String(error));
  } finally {
    busy.value = false;
  }
}


async function interrupt() {
  if (!canInterrupt.value) return;
  try {
    await invoke("interrupt", { id: props.session.id });
  } catch (e: any) {
    emit("error", e?.message ?? String(e));
  }
}

async function openInZed() {
  try {
    // Post-recovery, the fresh checkout path wins over the stale session view
    // (`recover_worktree` returned a path the server just materialized).
    await invoke("open_in_zed", {
      id: props.session.id,
      ...(zedTarget.value && !props.session.worktree_present
        ? { workDir: zedTarget.value }
        : {}),
    });
  } catch (e: any) {
    emit("error", e?.message ?? String(e));
  }
}

const repoUrl = computed(() => {
  const slug = props.session.github_repo;
  return slug && /^[\w.-]+\/[\w.-]+$/.test(slug)
    ? `https://github.com/${slug}`
    : null;
});

const prUrl = computed(() => {
  const cached = props.session.branch.github?.pr_url;
  if (cached) {
    try {
      const url = new URL(cached);
      if (url.protocol === "https:" && url.hostname === "github.com") return url.href;
    } catch { /* fall through to mapped number */ }
  }
  const number = props.session.branch.github_pr;
  return repoUrl.value && number && number > 0
    ? `${repoUrl.value}/pull/${number}`
    : null;
});

async function openResource(url: string) {
  try {
    await open(url);
  } catch (e: any) {
    emit("error", e?.message ?? String(e));
  }
}

// --- Code access ------------------------------------------------------------
//
// The spec's invariant: "If Arachne shows something referring to code, it
// should be possible to reach an editable checkout of that code in one
// action." The header keeps that action visible and puts repo, PR, branch,
// path, Terminal, and change details in its adjacent disclosure. When the
// worktree is gone (archive keeps the branch, not the directory), Open-in-Zed
// becomes Recover: `repos.worktrees.ensure` materializes a fresh checkout
// server-side, and the path updates to it.

const pr = computed(() => props.session.branch.github ?? null);

// The repo label: the managed slug when launched against one, else the
// basename of the checkout's repo root.
const repoLabel = computed(() => {
  if (props.session.github_repo) return props.session.github_repo;
  const root = props.session.branch.repo_root || "";
  return root ? root.split("/").filter(Boolean).slice(-2).join("/") : "";
});

// What Open-in-Zed opens: the live work_dir, or nothing when the checkout
// is gone (recovery replaces it).
const zedPath = computed(() =>
  props.session.worktree_present ? props.session.work_dir : "",
);

const recovering = ref(false);
const recoveredPath = ref<string | null>(null);

async function recover() {
  if (recovering.value) return;
  recovering.value = true;
  try {
    const wt = await invoke<{ path: string; created: boolean }>(
      "recover_worktree",
      {
        repoRoot: props.session.branch.repo_root,
        branch: props.session.branch.branch,
      },
    );
    recoveredPath.value = wt.path;
    await invoke("open_in_zed", { id: props.session.id, workDir: wt.path });
  } catch (e: any) {
    emit("error", e?.message ?? String(e));
  } finally {
    recovering.value = false;
  }
}

// The effective path Open-in-Zed acts on: recovered overrides the stale
// session view (the view is not refetched mid-session; the recovery result
// is fresher).
const zedTarget = computed(() => recoveredPath.value ?? zedPath.value);

function openTerminal() {
  // The terminal action: shell out to macOS Terminal on the same checkout.
  // A no-op when the checkout is gone — recover first.
  if (!zedTarget.value) return;
  invoke("open_in_terminal", { path: zedTarget.value }).catch((e: any) =>
    emit("error", e?.message ?? String(e)),
  );
}

// Archive lives in App.vue (single shared path with the sidebar row
// button): this component just forwards the request.
async function archive() {
  emit("archive", props.session.id);
}

// Thoughts and tool calls start collapsed; the header always shows the
// one-line summary so nothing is hidden.
function isCollapsed(key: string): boolean {
  return collapsed.value[key] ?? true;
}
function toggle(key: string) {
  collapsed.value[key] = !isCollapsed(key);
}
function workCollapsed(memberKeys: string[]): boolean {
  return !memberKeys.some((key) => collapsed.value[`tools:${key}`] === false);
}

// Markdown-preserving copy. WebKit's default copy from the rendered chat
// HTML writes text/plain as the selection's textContent — structure lost:
// bullets become •, backticks vanish, tables flatten. Message bodies keep
// their raw markdown on the rendered element (data-markdown), so on `copy`
// we rewrite the flavors: text/plain = the raw markdown of the selected
// bodies (pasting into GitHub keeps the source), text/html = the rendered
// sanitized markup (pasting into rich text keeps formatting). Selections
// spanning several messages join with a blank line; chrome inside the
// selection is ignored for the markdown flavor.
function onConversationCopy(event: ClipboardEvent) {
  const selection = document.getSelection();
  const container = convEl.value;
  if (!selection || selection.isCollapsed || !container || !event.clipboardData) return;
  const range = selection.getRangeAt(0);
  if (!container.contains(range.commonAncestorContainer)) return;
  const parts: string[] = [];
  for (const body of Array.from(container.querySelectorAll<HTMLElement>(".chat-markdown"))) {
    if (!range.intersectsNode(body)) continue;
    const md = body.getAttribute("data-markdown");
    if (md == null || !md.trim()) continue;
    // Partial selections clip the markdown to what was selected; whole
    // bodies copy exactly.
    parts.push(markdownForSelection(md, body, bodyOffset(range, body, "start"), bodyOffset(range, body, "end")));
  }
  const markdown = parts.map((part) => part.trim()).filter(Boolean).join("\n\n");
  if (!markdown) return;
  event.preventDefault();
  event.clipboardData.setData("text/plain", markdown);
  const html = htmlForSelection(selection);
  if (html) event.clipboardData.setData("text/html", html);
}

// Serialize the selection to HTML, reusing the browser's serialization of
// the already-sanitized rendered markup so rich-text paste keeps formatting.
function htmlForSelection(selection: Selection): string {
  const div = document.createElement("div");
  for (let i = 0; i < selection.rangeCount; i++) {
    div.appendChild(selection.getRangeAt(i).cloneContents());
  }
  return div.innerHTML;
}

function toggleWork(memberKeys: string[]) {
  const next = !workCollapsed(memberKeys);
  for (const key of memberKeys) collapsed.value[`tools:${key}`] = next;
}

// Normal/cancelled boundaries are transcript plumbing. Exceptional ACP stop
// reasons are user-visible outcomes: without this, a max-token turn looks like
// the agent simply went quiet (especially when its only output was thought).
function turnEndMessage(reason?: string): string | null {
  switch (reason) {
    case "max_tokens":
      return "The agent hit its response token limit before it could finish. Send a follow-up to continue.";
    case "max_turn_requests":
      return "The agent reached its turn-request limit before it could finish.";
    case "refusal":
      return "The agent refused this request.";
    case "error":
      return "The agent turn failed. Recover the session or send a follow-up to retry.";
    default:
      return null;
  }
}

// Loom appends an orientation note to the launch goal
// (loom-launch/provision.rs `entrance_note`, joined by a blank line —
// build_launch_prompt parts.join("\n\n")). Split the goal from the note so
// the goal leads and the boilerplate can collapse. Older or hand-typed
// messages without the marker return the whole text as the goal.
const ENTRANCE_MARKER = "You are working in a Loom session.";
function splitEntrance(text: string): { goal: string; entrance: string | null } {
  const idx = text.indexOf(ENTRANCE_MARKER);
  if (idx === -1) return { goal: text, entrance: null };
  return {
    goal: text.slice(0, idx).replace(/\s+$/, ""),
    entrance: text.slice(idx).replace(/^\s+/, ""),
  };
}

// The clipboard gets what the bubble shows: the goal plus the collapsed
// orientation note, not the goal alone.
function messageCopyText(block: ChatDisplayBlock): string {
  const { goal, entrance } = splitEntrance(block.text ?? "");
  return entrance ? `${goal}\n\n${entrance}` : goal;
}

function messageAuthor(block: ChatDisplayBlock): string {
  if (block.kind !== "user_message") return props.session.agent_kind;
  if (block.by?.startsWith("channel:")) return `via Loom · ${block.by.slice(8)}`;
  return "you";
}

const currentSummary = computed(() => props.fleet.find((s) => s.id === props.session.id));
const isWorker = computed(() => !!(currentSummary.value?.parent_session_id || currentSummary.value?.parent_id));
const lastIntegration = computed(() => currentSummary.value?.branch.tags.find((tag) => tag.key === "integration_result"));
const integrationTarget = computed(() => reviewIntegrationTarget(props.fleet, props.session.id));
// Every root conversation is a topic, including older single-prompt launches
// that predate the durable marker.
const isTopic = computed(() => !isWorker.value && !isStandalone(liveSummary.value));
const converting = ref(false);
async function convertToTrack() {
  converting.value = true;
  try { await invoke("convert_to_track", { id: props.session.id }); emit("refresh", props.session.id); }
  catch (error: any) { emit("error", error?.message ?? String(error)); }
  finally { converting.value = false; }
}
const sessionRepo = computed(() => props.session.github_repo || props.session.branch.repo_root);
const allIntegrateOptions = [
  { value: "squash", label: "Squash into track" },
  { value: "merge", label: "Merge into track" },
  { value: "rebase", label: "Rebase onto track" },
  { value: "cherry-pick", label: "Cherry-pick commits" },
  { value: "open-pr", label: "Open PR into track" },
  { value: "ask", label: "Ask coordinator to decide" },
];
const integrateOptions = computed(() => allIntegrateOptions.filter((option) =>
  option.value !== "cherry-pick" || workSummary.value?.has_commits === true,
));
// The deterministic fast path: no agent turn, Arachne squash-merges the
// topic into the primary checkout's current branch itself. Loom's API has
// no git writes, so Arachne runs git itself — valid only when the server
// is loopback, since its checkout paths are then local paths.
const loomIsLocal = computed(() => {
  try {
    const host = new URL(props.loomUrl).hostname;
    return ["localhost", "127.0.0.1", "::1", "[::1]"].includes(host);
  } catch {
    return false;
  }
});
const landOptions = computed(() => [
  { value: "open-pr", label: "Open PR" },
  { value: "squash", label: "Squash into upstream" },
  { value: "merge", label: "Merge into upstream" },
  { value: "rebase", label: "Rebase / fast-forward" },
  { value: "push", label: "Push track branch" },
  ...(loomIsLocal.value ? [{ value: "land-locally", label: "Squash into local checkout" }] : []),
]);
interface IntegrationTarget {
  coordinator_id: string;
  target_branch: string;
  coordinator_name: string;
}
interface LandResult {
  local: boolean;
  landing?: {
    landed: boolean;
    target_branch: string;
    commit?: string;
    commits_squashed: number;
    primary_checkout: string;
  };
}
const integrating = ref(false);
const landing = ref(false);
const integrationNote = ref("");
const showChanges = ref(false);
const workSummary = ref<{ files: number; additions: number; deletions: number; has_commits: boolean } | null>(null);
let workSummaryGeneration = 0;
async function refreshWorkSummary(id = props.session.id) {
  if (id !== props.session.id) return;
  const generation = ++workSummaryGeneration;
  try {
    const summary = await invoke<{ files: number; additions: number; deletions: number; has_commits: boolean }>("work_summary", { sessionId: id });
    if (props.session.id === id && generation === workSummaryGeneration) workSummary.value = summary;
  } catch {
    // A checkout may have been archived; don't retain counts from an older diff.
    if (props.session.id === id && generation === workSummaryGeneration) workSummary.value = null;
  }
}
watch(() => props.session.id, (id) => {
  showChanges.value = false;
  workSummary.value = null;
  void refreshWorkSummary(id);
}, { immediate: true });
async function onIntegrate(strategy: string) {
  if (integrating.value) return;
  integrating.value = true;
  integrationNote.value = "";
  try {
    const target = await invoke<IntegrationTarget>("integrate_session", { sessionId: props.session.id, strategy });
    localStorage.setItem(`arachne:strategy:integrate:${sessionRepo.value}`, strategy);
    integrationNote.value = `Sent to ${target.coordinator_name} (${target.target_branch}). Follow the integration in that thread.`;
  } catch (e: any) {
    emit("error", e?.message ?? String(e));
  } finally {
    integrating.value = false;
  }
}
async function onLand(strategy: string) {
  if (landing.value) return;
  landing.value = true;
  integrationNote.value = "";
  try {
    const result = await invoke<LandResult>("land_topic", { sessionId: props.session.id, strategy });
    localStorage.setItem(`arachne:strategy:land:${sessionRepo.value}`, strategy);
    if (result.local && result.landing) {
      const { landing: done } = result;
      integrationNote.value = done.landed
        ? `Landed locally: squash of ${done.commits_squashed} commit${done.commits_squashed === 1 ? "" : "s"} into ${done.target_branch} · ${done.commit}`
        : `${done.target_branch} already contained this track's changes — nothing to do.`;
    } else {
      integrationNote.value = "Landing request sent. Follow the result in this thread.";
    }
  } catch (e: any) {
    emit("error", e?.message ?? String(e));
  } finally {
    landing.value = false;
  }
}
</script>

<template>
  <section class="main">
    <div v-if="topic" class="thread-breadcrumb">
      <button @click="emit('home')">Tracks home</button><span> → </span>
      <button @click="emit('open-topic', topic.id)">{{ topic.branch.title || topic.branch.name }}</button>
      <span> → {{ session.branch.title || session.branch.name }}</span>
    </div>
    <div class="thread-header">
      <div class="meta">
        <div class="name">{{ session.branch.title || session.branch.name || session.id }}</div>
        <div class="sub">
          {{ session.agent_kind }} · {{ session.model || "auto" }} · turn
          {{ session.turn_count }}
        </div>
      </div>
      <!-- The scoped dashboard (Needs You / Working / Ready to Integrate) is an
           explicit detour, not the default Track view — the main pane stays a
           conversation whenever a Track is open. -->
      <button v-if="topic" title="Open this track's dashboard (Needs You, Working, Ready to Integrate)" @click="emit('overview', topic.id)">Overview</button>
      <!-- The explicit back-to-coordinator action (docs/design.md
           "Navigation"): from a worker thread, jump to the track's
           coordinator chat without remembered-thread restoration. -->
      <button
        v-if="topic && session.id !== topic.id"
        title="Back to this track's coordinator thread"
        @click="emit('open-coordinator', topic.id)"
      >Coordinator</button>
      <button
        v-if="canDismiss"
        class="accent-dismiss"
        title="Dismiss attention — clear this thread's flag"
        :disabled="dismissing"
        @click="dismissAttention"
      >{{ dismissing ? "Dismissing…" : "Dismiss" }}</button>
      <CodeActions
        :checkout-path="zedTarget"
        :recovering="recovering"
        :repo-label="repoLabel"
        :repo-url="repoUrl"
        :pr-url="prUrl"
        :pr-number="session.branch.github?.pr_number || session.branch.github_pr || null"
        :pr-draft="pr?.is_draft"
        :pr-state="pr?.pr_state"
        :review-decision="pr?.review_decision"
        :checks="pr?.checks"
        :branch="session.branch.branch"
        :integration-branch="isWorker && integrationTarget ? integrationTarget.branch.branch : null"
        :work-summary="workSummary"
        @open-checkout="zedTarget ? openInZed() : recover()"
        @open-terminal="openTerminal"
        @open-resource="openResource"
      />
      <button :disabled="!session.work_dir" :aria-expanded="showChanges" @click="showChanges = !showChanges">{{ showChanges ? "Hide diff" : "Review diff" }}</button>
      <SplitButton v-if="isWorker" kind="integrate" :repo="sessionRepo" :options="integrateOptions" label="Integrate" :busy="integrating" :disabled="!integrationTarget" @run="onIntegrate" />
      <SplitButton v-if="isTopic" kind="land" :repo="sessionRepo" :options="landOptions" label="Land" :busy="landing" :disabled="session.status === 'archived'" @run="onLand" />
      <button v-if="!isWorker && !isTopic" :disabled="converting || session.status === 'archived'" @click="convertToTrack">{{ converting ? 'Converting…' : 'Convert to track' }}</button>
      <button @click="interrupt">Interrupt</button>
      <button class="danger" @click="archive">Archive</button>
      <button class="accent" @click="showDelegate ? showDelegate = false : openDelegate()">
        Delegate
      </button>
      <button @click="showSendToThread = !showSendToThread">Send to thread…</button>
    </div>
    <div v-if="integrationNote" class="integrate-note">{{ integrationNote }}</div>
    <div v-if="isWorker && lastIntegration" class="integrate-note" :title="lastIntegration.note">
      Last integrated result: {{ lastIntegration.value }}<span v-if="lastIntegration.note"> · {{ lastIntegration.note }}</span>
    </div>
    <div v-if="showSendToThread" class="send-thread-box">
      <label>
        <span>Destination</span>
        <select v-model="sendDestination">
          <option value="" disabled>Choose a thread</option>
          <option v-for="s in destinations" :key="s.id" :value="s.id">
            {{ s.branch.title || s.branch.name }}
          </option>
        </select>
      </label>
      <textarea v-model="sendNote" placeholder="Note to deliver with this thread as the source…" />
      <div class="send-thread-actions">
        <button @click="showSendToThread = false">Cancel</button>
        <button class="primary" :disabled="!sendDestination || !sendNote.trim() || sendingToThread" @click="sendThreadNote">
          {{ sendingToThread ? "Sending…" : "Send note" }}
        </button>
      </div>
    </div>
    <ChangeReview v-if="showChanges" :session-id="session.id" :can-edit="loomIsLocal" :is-topic="isTopic" :integration-target="integrationTarget" @saved="refreshWorkSummary()" />
    <div v-if="showDelegate" class="delegate-box">
      <input
        ref="delegateInput"
        v-model="delegateTask"
        placeholder="child task… goal, scope, and checks — e.g. “add retries to fetchBlobs in src/api.ts; cover with tests in fetch.test.ts; run npm test”"
        @keydown.enter.prevent="submitDelegate"
        @keydown.esc="showDelegate = false"
      />
      <LaunchPreset />
      <button
        class="primary"
        :disabled="!delegateTask.trim() || delegating"
        @click="submitDelegate"
      >
        {{ delegating ? "Delegating…" : "Start worker" }}
      </button>
    </div>
    <div class="conversation" ref="convEl" @scroll="onConversationScroll" @copy="onConversationCopy">
      <div v-if="hasOlder" class="load-older">
        <button :disabled="loadingOlder" @click="loadOlder">
          {{ loadingOlder ? "loading…" : "load older" }}
        </button>
      </div>
      <template v-for="row in rows" :key="row.key">
        <!-- Consecutive tool calls and finished thinking in one turn share one
             disclosure. -->
        <div v-if="row.kind === 'work_group'" class="block tool" :data-copy-host="row.key">
          <div class="tool-line" role="button" tabindex="0"
            :aria-expanded="!workCollapsed(row.memberKeys)"
            @click="toggleWork(row.memberKeys)"
            @keydown.enter.prevent="toggleWork(row.memberKeys)"
            @keydown.space.prevent="toggleWork(row.memberKeys)">
            <span class="status" :class="{ running: row.state === 'thinking' }">{{ row.state }}</span>
            <span class="tool-title">worked</span>
            <span class="tool-summary">{{
              [
                row.calls.length ? `${row.calls.length} ${row.calls.length === 1 ? 'tool call' : 'tool calls'}` : null,
                row.thinkingTokens ? `~${formatTokens(row.thinkingTokens)} thinking tokens` : null,
              ].filter(Boolean).join(' · ') || 'done'
            }}</span>
            <span class="chevron">{{ workCollapsed(row.memberKeys) ? "▸" : "▾" }}</span>
          </div>
          <CopyButton :corner="cornerOf(row.key)" :text="row.blocks.map((member) => member.kind === 'tool_call' ? toolCallCopyText(member) : (member.text ?? '')).join('\n\n')" :label="`Copy ${row.blocks.length === 1 ? 'block' : row.blocks.length + ' blocks'}`" />
          <div v-if="!workCollapsed(row.memberKeys)" class="tool-group-detail">
            <div v-for="(member, j) in row.blocks" :key="row.memberKeys[j]" class="tool-detail" :data-copy-host="`detail:${row.memberKeys[j]}`">
              <template v-if="member.kind === 'tool_call'">
                <span class="status" :class="{ running: member.status === 'running' }">{{ member.status }}</span>
                <strong>{{ member.title || member.tool_kind || 'tool' }}</strong>
                <CopyButton :corner="cornerOf(`detail:${row.memberKeys[j]}`)" :text="toolCallCopyText(member)" :label="`Copy ${member.title || member.tool_kind || 'tool call'}`" />
                <div v-if="member.summary">{{ member.summary }}</div>
              </template>
              <template v-else>
                <span class="tool-summary">{{ member.summary || (member.text ?? '').slice(0, 160) }}</span>
                <div class="thought-body"><ChatMarkdown :text="member.text ?? ''" /></div>
              </template>
              <ChatImages :block="member" :session-id="session.id" />
            </div>
          </div>
        </div>
        <template v-else>
        <!-- The live turn's trailing thought renders open while it is being
             thought; it folds into the preceding group once the turn ends. -->
        <template v-if="row.kind === 'live_thought'">
          <div class="block thought">
            <div class="tool-line" role="button" tabindex="0"
              :aria-expanded="!isCollapsed(row.key)"
              @click="toggle(row.key)"
              @keydown.enter.prevent="toggle(row.key)"
              @keydown.space.prevent="toggle(row.key)">
              <span class="status running">thinking</span>
              <span class="tool-summary">{{ row.block.summary || (row.block.text ?? '').slice(0, 160) }}</span>
            </div>
            <div v-if="!isCollapsed(row.key)" class="body thought-body">
              <ChatMarkdown :text="row.block.text ?? ''" />
            </div>
          </div>
        </template>
        <template v-else-if="row.block.kind === 'thought'">
          <div class="block thought" :data-copy-host="row.key">
            <div class="tool-line" role="button" tabindex="0"
              :aria-expanded="!isCollapsed(row.key)"
              @click="toggle(row.key)"
              @keydown.enter.prevent="toggle(row.key)"
              @keydown.space.prevent="toggle(row.key)">
              <span class="tool-title">thinking</span>
              <span class="tool-summary">{{ row.block.summary || (row.block.text ?? '').slice(0, 160) }}</span>
              <span class="chevron">{{ isCollapsed(row.key) ? "▸" : "▾" }}</span>
            </div>
            <CopyButton :corner="cornerOf(row.key)" :text="blockCopyText(row.block)" label="Copy thinking" />
            <div v-if="!isCollapsed(row.key)" class="body thought-body">
              <ChatMarkdown :text="row.block.text ?? ''" />
            </div>
          </div>
        </template>
        <div v-else-if="row.block.kind === 'permission_request'" class="block permission-request" role="group" :aria-label="`Tool approval: ${row.block.title || 'permission requested'}`">
          <div class="who">Tool approval</div>
          <div class="body">
            <div class="permission-title">{{ row.block.title || 'Permission requested' }}</div>
            <div v-if="row.block.outcome" class="permission-receipt">
              {{ row.block.outcome.cancelled ? 'Cancelled' : `Answered: ${row.block.outcome.option_id || 'resolved'}` }}
            </div>
            <div v-else class="permission-options">
              <button v-for="option in row.block.options ?? []" :key="option.option_id"
                :disabled="answeringPermission === row.block.request_id"
                @click="answerPermission(row.block, option.option_id)">{{ option.name }}</button>
            </div>
            <div v-if="row.block.request_id && permissionError[row.block.request_id]" class="permission-error" role="alert">{{ permissionError[row.block.request_id] }}</div>
          </div>
        </div>
        <!-- Plans -->
        <div v-else-if="row.block.kind === 'plan'" class="block" :data-copy-host="row.key">
          <div class="who">plan</div>
          <div class="body">
            <div v-for="(entry, j) in row.block.entries" :key="j">
              [{{ entry[1] }}] {{ entry[0] }}
            </div>
          </div>
          <CopyButton :corner="cornerOf(row.key)" :text="blockCopyText(row.block)" label="Copy plan" />
        </div>
        <!-- User / agent messages. A slash-command reply (pi-acp's interceptor
             answered locally — stats, receipts, confirmations) renders as a
             system message, distinct from agent prose. -->
        <div
          v-else-if="row.block.kind === 'user_message' || row.block.kind === 'agent_message'"
          class="block"
          :class="{
            user: row.block.kind === 'user_message',
            agent: row.block.kind === 'agent_message' && !isCommandReply(row.block),
            system: isCommandReply(row.block),
          }"
          :data-copy-host="row.key"
        >
          <div class="who">
            {{ isCommandReply(row.block) ? `system · /${commandTurns.get(row.block.turn ?? -1)}` : messageAuthor(row.block) }}
          </div>
          <CopyButton :corner="cornerOf(row.key)" :text="messageCopyText(row.block)" :label="`Copy ${row.block.kind === 'user_message' ? 'message' : isCommandReply(row.block) ? 'output' : 'reply'}`" />
          <!-- Loom's orientation note (goal + "You are working in a Loom
               session…") is real prompt text the agent saw — keep it in the
               transcript, but collapse the boilerplate behind a disclosure
               so the goal leads. -->
          <template v-if="row.block.kind === 'user_message' && splitEntrance(row.block.text ?? '').entrance">
            <div v-if="splitEntrance(row.block.text ?? '').goal" class="body">
              <ChatMarkdown :text="splitEntrance(row.block.text ?? '').goal" />
            </div>
            <div class="entrance">
              <div class="entrance-line" role="button" tabindex="0"
                :aria-expanded="!isCollapsed(row.key)"
                @click="toggle(row.key)"
                @keydown.enter.prevent="toggle(row.key)"
                @keydown.space.prevent="toggle(row.key)">
                <span class="tool-summary">loom orientation</span>
                <span class="chevron">{{ isCollapsed(row.key) ? "▸" : "▾" }}</span>
              </div>
              <div v-if="!isCollapsed(row.key)" class="body entrance-body">
                <ChatMarkdown :text="splitEntrance(row.block.text ?? '').entrance ?? ''" />
              </div>
            </div>
          </template>
          <div v-else-if="isCommandReply(row.block)" class="body">{{ row.block.text ?? "" }}</div>
          <div v-else class="body"><ChatMarkdown :text="row.block.text ?? ''" /></div>
          <ChatImages :block="row.block" :session-id="session.id" />
        </div>
        <!-- Exceptional turn boundaries must stay visible. In particular,
             pi's `length` maps to ACP `max_tokens`: hiding it makes a turn
             containing only reasoning look like a silent success. -->
        <div
          v-else-if="row.block.kind === 'turn_end' && turnEndMessage(row.block.stop_reason)"
          class="block turn-error"
          role="alert"
        >
          <div class="who">turn stopped · {{ row.block.stop_reason }}</div>
          <div class="body">{{ turnEndMessage(row.block.stop_reason) }}</div>
        </div>
        <!-- usage / normal turn_end / unknown: no visual block -->
        </template>
      </template>
      <!-- A queued prompt can outlive its agent. Only a live turn spins. -->
      <div v-if="(turnLive && currentStatus === 'running') || pendingPrompt" class="block working">
        <div class="who">{{ session.agent_kind }}</div>
        <div class="body working-body">
          <span v-if="turnLive && currentStatus === 'running'" class="spinner" aria-hidden="true"></span>
          <span class="working-label">{{
            turnLive && currentStatus === 'running'
              ? (pendingPrompt ? 'Working — message queued…' : 'Working…')
              : 'Message queued…'
          }}</span>
          <span v-if="turnLive && currentStatus === 'running' && elapsedLabel" class="working-meta">{{
            `${elapsedLabel} elapsed`
          }}</span>
          <span
            v-if="turnLive && currentStatus === 'running' && progressAge >= 15"
            class="working-meta quiet"
            title="No visible output for a while — the model may be reasoning without streaming."
            >no updates for {{ progressAge }}s</span
          >
        </div>
      </div>
      <div v-if="blocks.length === 0" style="color: var(--text-dim)">
        No conversation yet.
      </div>
    </div>
    <div class="composer-wrap" @dragover.prevent @drop.prevent="($event) => $event.dataTransfer?.files && addFiles($event.dataTransfer.files)">
      <div v-if="showHandoff" class="handoff-box composer-handoff">
        <div class="handoff-heading">Switch this thread’s runtime</div>
        <div class="handoff-fields">
          <label>Profile
            <select v-model="handoffProfile" @change="chooseHandoffProfile">
              <option v-for="p in launchOptions?.profiles ?? []" :key="p.name" :value="p.name">{{ p.name }}</option>
            </select>
          </label>
          <label>Agent
            <select v-model="handoffAgent" @change="chooseHandoffAgent">
              <option value="">Profile default</option>
              <option v-for="a in launchOptions?.agents ?? []" :key="a.kind" :value="a.kind">{{ a.label }}</option>
            </select>
          </label>
          <label>Model
            <select v-if="handoffAgentChoice && !handoffAgentChoice.accepts_raw_model && handoffAgentChoice.models.length" v-model="handoffModel">
              <option value="">Agent default</option>
              <option v-for="m in handoffAgentChoice.models" :key="m.id" :value="m.id">{{ m.label }}</option>
            </select>
            <input v-else v-model="handoffModel" placeholder="Agent default" spellcheck="false" />
          </label>
          <label>Effort
            <select v-model="handoffEffort">
              <option value="">Profile default</option>
              <option v-for="e in handoffAgentChoice?.efforts ?? []" :key="e.id" :value="e.id">{{ e.label }}</option>
            </select>
          </label>
        </div>
        <div class="handoff-actions">
          <span>Switching restarts the agent while keeping this thread and checkout.</span>
          <button @click="showHandoff = false">Cancel</button>
          <button class="primary" :disabled="!handoffAllowed || handingOff" @click="handoff">{{ handingOff ? "Switching…" : "Switch" }}</button>
        </div>
      </div>
      <div v-if="mentionRange" class="mention-menu" role="listbox" aria-label="Track resources">
        <div v-if="mentionLoading" class="mention-hint">Loading resources…</div>
        <div v-else-if="mentionError" class="mention-hint">{{ mentionError }}</div>
        <div v-else-if="!matchingResources.length" class="mention-hint">No matching attached resources</div>
        <button v-for="(resource, index) in matchingResources" :key="resource.id" role="option"
          :aria-selected="index === mentionIndex" :class="{ selected: index === mentionIndex }"
          @mousedown.prevent="chooseMention(resource)">
          <strong>{{ resource.title }}</strong>
          <small>{{ resource.path || resource.url || resource.reference || resource.kind }}</small>
        </button>
      </div>
      <div v-if="slash.matches.value.length" class="mention-menu slash-menu" role="listbox" aria-label="Agent commands">
        <button v-for="(command, index) in slash.matches.value" :key="command.name" type="button" role="option"
          :aria-selected="index === slash.selected.value" :class="{ selected: index === slash.selected.value }"
          @click="slash.choose(command)">
          <strong>/{{ command.name }}</strong><small>{{ command.description }}</small>
        </button>
      </div>
      <div class="composer-card">
      <div v-if="attachments.length || attachmentError || attachmentLoading" class="attachment-row composer-attachments">
        <span v-for="(file, index) in attachments" :key="file.name" class="attachment-chip">
          <img v-if="imagePreviewUrl(file)" :src="imagePreviewUrl(file)!" class="attachment-preview" alt="" />
          {{ file.name }} <button type="button" :aria-label="`Remove ${file.name}`" @click="attachments.splice(index, 1)">×</button>
        </span>
        <span v-if="attachmentError" class="attachment-error">{{ attachmentError }}</span>
        <span v-if="attachmentLoading" class="attachment-hint">Reading files…</span>
      </div>
      <div class="composer">
      <div class="file-completion-anchor">
        <textarea
          ref="composerEl"
          v-model="draft"
          :disabled="!canSend"
          :placeholder="
            turnLive
              ? 'Agent is working — your message will queue behind the current turn…'
              : `Ask ${session.agent_kind || 'the agent'} to implement, inspect, explain, or fix…${metadata?.commands.length ? ' Type / for commands.' : ''}`
          "
          @input="completion.updateCaret"
          @click="completion.updateCaret"
          @keyup="completion.updateCaret"
          @keydown="onComposerKeydown"
          @paste="onComposerPaste"
        ></textarea>
        <ul
          v-if="completion.visible.value"
          class="file-completion-menu above"
          role="listbox"
          aria-label="Worktree files"
        >
          <li v-for="(path, index) in completion.matches.value" :key="path">
            <button
              type="button"
              role="option"
              :aria-selected="index === completion.selected.value"
              @mousedown.prevent
              @click="completion.choose(path)"
            >
              @{{ path }}
            </button>
          </li>
        </ul>
      </div>
      </div>
      <div v-if="slash.hint.value" class="slash-argument-hint">{{ slash.hint.value }}</div>
      <div class="composer-utility-row">
        <div class="composer-context-slot"><ContextTelemetry :session-id="session.id" :usage="currentUsage" /></div>
        <button v-if="canCompact" class="composer-icon-button compact-button" type="button"
          :disabled="!canSend || turnLive || busy" title="Compact context" aria-label="Compact context" @click="compactContext">↘</button>
        <label v-if="modelConfig" class="composer-pill model-pill" :title="modelConfig.name">
          <span aria-hidden="true">▣</span>
          <select :value="String(modelConfig.currentValue)" :disabled="!!configBusy" aria-label="Model" @change="setConfig(modelConfig, $event)">
            <option v-for="choice in configChoices(modelConfig)" :key="choice.value" :value="choice.value">{{ choice.name }}</option>
          </select>
          <span aria-hidden="true">⌄</span>
        </label>
        <button v-else-if="session.protocol === 'acp'" class="composer-pill model-pill" type="button"
          :disabled="!handoffAllowed" :aria-expanded="showHandoff" title="Switch model when the session is idle"
          @click="showHandoff = !showHandoff">
          <span aria-hidden="true">▣</span><span class="composer-pill-label">{{ session.model || 'Agent default' }}</span><span aria-hidden="true">⌄</span>
        </button>
        <label v-if="effortConfig" class="composer-pill effort-pill" :title="effortConfig.name">
          <span aria-hidden="true">◉</span>
          <select :value="String(effortConfig.currentValue)" :disabled="!!configBusy" aria-label="Thinking effort" @change="setConfig(effortConfig, $event)">
            <option v-for="choice in configChoices(effortConfig)" :key="choice.value" :value="choice.value">Thinking: {{ choice.name.replace(/^thinking:\s*/i, "") }}</option>
          </select>
          <span aria-hidden="true">⌄</span>
        </label>
        <button v-else-if="session.protocol === 'acp'" class="composer-pill effort-pill" type="button"
          :disabled="!handoffAllowed" :aria-expanded="showHandoff" title="Switch thinking effort when the session is idle"
          @click="showHandoff = !showHandoff">
          <span aria-hidden="true">◉</span><span>Thinking: {{ session.effort || 'Default' }}</span><span aria-hidden="true">⌄</span>
        </button>
        <button v-if="repoUrl" class="composer-icon-button" type="button" :title="`Open ${session.github_repo} on GitHub`" aria-label="Open repository on GitHub" @click="openResource(repoUrl)">
          <svg viewBox="0 0 24 24" aria-hidden="true"><path fill="currentColor" d="M12 .8a11.2 11.2 0 0 0-3.54 21.82c.56.1.76-.24.76-.54v-1.9c-3.09.67-3.74-1.31-3.74-1.31-.51-1.28-1.24-1.62-1.24-1.62-1.01-.69.08-.68.08-.68 1.12.08 1.71 1.15 1.71 1.15 1 .1 1.66-.45 2.05-.82.1-.72.39-1.2.71-1.48-2.47-.28-5.07-1.24-5.07-5.53 0-1.22.44-2.22 1.15-3-.12-.28-.5-1.42.11-2.95 0 0 .94-.3 3.08 1.15A10.7 10.7 0 0 1 12 4.42c.95 0 1.9.13 2.79.38 2.14-1.45 3.08-1.15 3.08-1.15.61 1.53.23 2.67.11 2.95.72.78 1.15 1.78 1.15 3 0 4.3-2.6 5.25-5.08 5.53.4.35.76 1.02.76 2.06v2.89c0 .3.2.65.77.54A11.2 11.2 0 0 0 12 .8Z"/></svg>
        </button>
        <button v-else class="composer-icon-button" type="button" :disabled="recovering" :title="zedTarget ? 'Open checkout in Zed' : 'Recover checkout'" aria-label="Open repository checkout" @click="zedTarget ? openInZed() : recover()">⌂</button>
        <label class="attachment-pick composer-attach" title="Attach files or images" aria-label="Attach files or images">
          <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M8 12.5 14.5 6a3 3 0 0 1 4.25 4.25l-8.5 8.5a5 5 0 0 1-7.07-7.07l8.5-8.5" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/></svg>
          <input type="file" multiple :disabled="!canSend || attachmentLoading" aria-label="Attach files or images to message" @change="onFileInput" />
        </label>
        <button v-if="metadata?.commands.length" class="composer-icon-button slash-button" type="button" title="Browse agent commands" aria-label="Browse agent commands" @click="slash.show">/</button>
        <button
          class="composer-send"
          :class="{ 'send-dismiss': sendAsDismiss }"
          type="button"
          :disabled="sendAsDismiss ? dismissing : (!canSend || (!draft.trim() && !attachments.length) || busy || attachmentLoading)"
          :title="sendAsDismiss ? 'Dismiss attention — clear this thread\'s flag (⌘/Ctrl + Enter)' : busy ? 'Sending…' : 'Send message (⌘/Ctrl + Enter)'"
          :aria-label="sendAsDismiss ? 'Dismiss attention' : 'Send message'"
          @click="sendAsDismiss ? dismissAttention() : send()"
        >
          <span v-if="sendAsDismiss && dismissing">…</span>
          <svg v-else-if="sendAsDismiss" viewBox="0 0 24 24" aria-hidden="true"><path fill="currentColor" d="M9.55 17.6 4.4 12.45l1.6-1.6 3.55 3.55 8.5-8.5 1.6 1.6Z"/></svg>
          <span v-else-if="busy">…</span><span v-else aria-hidden="true">↑</span>
        </button>
      </div>
      </div>
      <div class="composer-hint">⌘/Ctrl + Enter to send · Enter for a new line</div>
    </div>
  </section>
</template>
