<script setup lang="ts">
import { ref, computed, onMounted, onUnmounted, nextTick, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { SessionView } from "../App.vue";
import ChatMarkdown from "./ChatMarkdown.vue";
import ChatImages from "./ChatImages.vue";
import { groupDisplayBlocks, type ChatDisplayBlock } from "../chatRows";

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
  live_turn: number | null;
  pending_prompt: string | null;
  live_started_at: string | null;
  live_progress_at: string | null;
}

// One `loom://chat-event` frame: {topic, event, data}. On the chat topic
// loom emits turn / block / delta / tool / queue (and resync when a
// bounded broadcast drops frames); on the session topic, status/tag kinds.
interface ChatEventFrame {
  topic: string;
  event: string;
  data?: any;
}

const props = defineProps<{ session: SessionView }>();
const emit = defineEmits<{
  (e: "error", msg: string): void;
  (e: "archive", id: string): void;
  (e: "delegate", parentId: string, task: string): void;
}>();

const blocks = ref<ChatDisplayBlock[]>([]);
const rows = computed(() => groupDisplayBlocks(blocks.value));
const draft = ref("");
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
  turnStartedAt.value = Date.now();
  lastProgressAt.value = Date.now();
  pendingPrompt.value = null;
  sseTurnAt = Date.now();
  if (atBottom()) scrollToBottom();
}

function turnEnded() {
  turnLive.value = false;
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
    turnStartedAt.value = null;
    lastProgressAt.value = null;
  }
}

function onChatFrame(frame: ChatEventFrame) {
  const d = frame.data ?? {};
  switch (frame.event) {
    case "turn":
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
const delegating = ref(false);

function submitDelegate() {
  const t = delegateTask.value.trim();
  if (!t || delegating.value) return;
  delegating.value = true;
  emit("delegate", props.session.id, t);
  delegateTask.value = "";
  showDelegate.value = false;
  delegating.value = false;
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

onMounted(async () => {
  ticker = setInterval(() => (clock.value = Date.now()), 1000);
  await reload();
  scrollToBottom();
  unlisteners.push(
    await listen("loom://chat-event", (event) => {
      const frame = event.payload as ChatEventFrame;
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
});

async function send() {
  const text = draft.value.trim();
  if (!text || busy.value) return;
  busy.value = true;
  try {
    await invoke("send_input", {
      id: props.session.id,
      text,
      protocol: props.session.protocol,
    });
    draft.value = "";
    await reload();
  } catch (e: any) {
    emit("error", e?.message ?? String(e));
  } finally {
    busy.value = false;
  }
}

async function interrupt() {
  try {
    await invoke("interrupt", { id: props.session.id });
  } catch (e: any) {
    emit("error", e?.message ?? String(e));
  }
}

async function openInZed() {
  try {
    await invoke("open_in_zed", { workDir: props.session.work_dir });
  } catch (e: any) {
    emit("error", e?.message ?? String(e));
  }
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
function toolsCollapsed(memberKeys: string[]): boolean {
  return !memberKeys.some((key) => collapsed.value[`tools:${key}`] === false);
}
function toggleTools(memberKeys: string[]) {
  const next = !toolsCollapsed(memberKeys);
  for (const key of memberKeys) collapsed.value[`tools:${key}`] = next;
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
</script>

<template>
  <section class="main">
    <div class="thread-header">
      <div class="meta">
        <div class="name">{{ session.branch.name || session.id }}</div>
        <div class="sub">
          {{ session.agent_kind }} · {{ session.model || "auto" }} · turn
          {{ session.turn_count }} ·
          {{ session.work_dir }}
        </div>
      </div>
      <button @click="openInZed">Open in Zed</button>
      <button @click="interrupt">Interrupt</button>
      <button class="danger" @click="archive">Archive</button>
      <button class="accent" @click="showDelegate = !showDelegate">
        Delegate
      </button>
    </div>
    <div v-if="showDelegate" class="delegate-box">
      <input
        v-model="delegateTask"
        placeholder="child task… e.g. “run the tests and report failures”"
        @keydown.enter.prevent="submitDelegate"
        @keydown.esc="showDelegate = false"
      />
      <button
        class="primary"
        :disabled="!delegateTask.trim()"
        @click="submitDelegate"
      >
        Spawn child
      </button>
    </div>
    <div class="conversation" ref="convEl">
      <div v-if="hasOlder" class="load-older">
        <button :disabled="loadingOlder" @click="loadOlder">
          {{ loadingOlder ? "loading…" : "load older" }}
        </button>
      </div>
      <template v-for="row in rows" :key="row.key">
        <!-- Consecutive calls in one turn share one disclosure. -->
        <div v-if="row.kind === 'tool_call_group'" class="block tool">
          <div class="tool-line" role="button" tabindex="0"
            :aria-expanded="!toolsCollapsed(row.memberKeys)"
            @click="toggleTools(row.memberKeys)"
            @keydown.enter.prevent="toggleTools(row.memberKeys)"
            @keydown.space.prevent="toggleTools(row.memberKeys)">
            <span class="status" :class="{ running: row.blocks.some((call) => call.status === 'running') }">{{
              row.blocks.some((call) => call.status === 'running') ? 'running' : 'done'
            }}</span>
            <span class="tool-title">tool calls</span>
            <span class="tool-summary">{{ row.blocks.length }} {{ row.blocks.length === 1 ? 'call' : 'calls' }}</span>
            <span class="chevron">{{ toolsCollapsed(row.memberKeys) ? "▸" : "▾" }}</span>
          </div>
          <div v-if="!toolsCollapsed(row.memberKeys)" class="tool-group-detail">
            <div v-for="(call, j) in row.blocks" :key="row.memberKeys[j]" class="tool-detail">
              <span class="status" :class="{ running: call.status === 'running' }">{{ call.status }}</span>
              <strong>{{ call.title || call.tool_kind || 'tool' }}</strong>
              <div v-if="call.summary">{{ call.summary }}</div>
              <ChatImages :block="call" :session-id="session.id" />
            </div>
          </div>
        </div>
        <template v-else>
        <template v-if="row.block.kind === 'thought'">
          <div class="block thought">
            <div class="tool-line" role="button" tabindex="0"
              :aria-expanded="!isCollapsed(row.key)"
              @click="toggle(row.key)"
              @keydown.enter.prevent="toggle(row.key)"
              @keydown.space.prevent="toggle(row.key)">
              <span class="tool-title">thinking</span>
              <span class="tool-summary">{{ row.block.summary || (row.block.text ?? '').slice(0, 160) }}</span>
              <span class="chevron">{{ isCollapsed(row.key) ? "▸" : "▾" }}</span>
            </div>
            <div v-if="!isCollapsed(row.key)" class="body thought-body">
              <ChatMarkdown :text="row.block.text ?? ''" />
            </div>
          </div>
        </template>
        <!-- Plans -->
        <div v-else-if="row.block.kind === 'plan'" class="block">
          <div class="who">plan</div>
          <div class="body">
            <div v-for="(entry, j) in row.block.entries" :key="j">
              [{{ entry[1] }}] {{ entry[0] }}
            </div>
          </div>
        </div>
        <!-- User / agent messages -->
        <div
          v-else-if="row.block.kind === 'user_message' || row.block.kind === 'agent_message'"
          class="block"
          :class="{
            user: row.block.kind === 'user_message',
            agent: row.block.kind === 'agent_message',
          }"
        >
          <div class="who">
            {{ row.block.kind === "user_message" ? "you" : session.agent_kind }}
          </div>
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
          <div v-else class="body"><ChatMarkdown :text="row.block.text ?? ''" /></div>
          <ChatImages :block="row.block" :session-id="session.id" />
        </div>
        <!-- usage / turn_end / unknown: no visual block -->
        </template>
      </template>
      <!-- Working indicator: shown while an ACP turn is live or a prompt is
           queued behind it. Sits at the tail of the log so it reads as the
           agent's next message being composed. -->
      <div v-if="turnLive || pendingPrompt" class="block working">
        <div class="who">{{ session.agent_kind }}</div>
        <div class="body working-body">
          <span class="spinner" aria-hidden="true"></span>
          <span class="working-label">{{
            pendingPrompt ? "working — message queued…" : "working…"
          }}</span>
          <span v-if="turnLive && elapsedLabel" class="working-meta">{{
            `${elapsedLabel} elapsed`
          }}</span>
          <span
            v-if="turnLive && progressAge >= 15"
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
    <div class="composer">
      <textarea
        v-model="draft"
        :placeholder="
          turnLive
            ? 'Agent is working — your message will queue behind the current turn…'
            : 'Message the agent…'
        "
        @keydown.enter.exact.prevent="send"
      ></textarea>
      <button class="primary" :disabled="!draft.trim() || busy" @click="send">
        {{ busy ? "…" : "Send" }}
      </button>
    </div>
  </section>
</template>
