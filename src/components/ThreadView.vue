<script setup lang="ts">
import { ref, onMounted, onUnmounted, nextTick, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { SessionView } from "../App.vue";

// DisplayBlock from src-tauri/src/blocks.rs, serialized internally-tagged:
// { kind: "user_message", text: "…", by: … } — flat fields keyed by `kind`.
interface DisplayBlock {
  kind:
    | "user_message"
    | "agent_message"
    | "thought"
    | "tool_call"
    | "plan"
    | "usage"
    | "turn_end"
    | "other";
  text?: string;
  by?: string | null;
  tool_kind?: string;
  title?: string;
  status?: string;
  summary?: string;
  entries?: [string, string][];
  used?: number | null;
  size?: number | null;
  stop_reason?: string;
  unknown_kind?: string;
  payload?: string;
}

interface Cursor {
  turn: number;
  seq: number;
}

const props = defineProps<{ session: SessionView }>();
const emit = defineEmits<{
  (e: "error", msg: string): void;
  (e: "archive", id: string): void;
  (e: "delegate", parentId: string, task: string): void;
}>();

const blocks = ref<DisplayBlock[]>([]);
const draft = ref("");
const busy = ref(false);
const loadingOlder = ref(false);
const hasOlder = ref(true);
const convEl = ref<HTMLElement | null>(null);
const unlisteners: UnlistenFn[] = [];

// Delegation: spawn a child session under this one. The child lands in the
// same workstream (loom inherits the parent's placement group) and nests
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
  try {
    blocks.value = await invoke<DisplayBlock[]>("fetch_chat", {
      id: props.session.id,
    });
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
    const older = await invoke<DisplayBlock[]>("fetch_chat", {
      id: props.session.id,
      beforeTurn: cursor.turn,
      beforeSeq: cursor.seq,
    });
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

// Collapsible sections (thoughts, tool runs): expanded state per block index.
const collapsed = ref<Record<number, boolean>>({});

onMounted(async () => {
  await reload();
  scrollToBottom();
  unlisteners.push(
    await listen("loom://chat-event", (event) => {
      const frame = event.payload as { topic: string; event: string };
      if (
        frame.topic.startsWith("chat:") ||
        frame.topic.startsWith("session:")
      ) {
        scheduleReload();
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
      await reload();
      scrollToBottom();
    }
  },
);

onUnmounted(() => {
  unlisteners.forEach((u) => u());
  if (reloadTimer) clearTimeout(reloadTimer);
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

async function archive() {
  try {
    await invoke("archive_session", { id: props.session.id });
    emit("archive", props.session.id);
  } catch (e: any) {
    emit("error", e?.message ?? String(e));
  }
}

// Thoughts and tool calls start collapsed; the header always shows the
// one-line summary so nothing is hidden.
function isCollapsed(i: number): boolean {
  return collapsed.value[i] ?? true;
}
function toggle(i: number) {
  collapsed.value[i] = !isCollapsed(i);
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
      <template v-for="(b, i) in blocks" :key="i">
        <!-- Tool calls: collapsed one-liner by default, expandable -->
        <div v-if="b.kind === 'tool_call'" class="block tool">
          <div class="tool-line" @click="toggle(i)">
            <span class="status" :class="{ running: b.status === 'running' }">{{
              b.status
            }}</span>
            <span class="tool-title">{{ b.title }}</span>
            <span v-if="b.summary" class="tool-summary">{{
              b.summary.slice(0, 200)
            }}</span>
            <span class="chevron">{{ isCollapsed(i) ? "▸" : "▾" }}</span>
          </div>
          <div v-if="!isCollapsed(i)" class="tool-detail">{{ b.summary }}</div>
        </div>
        <!-- Thoughts: collapsed italic one-liner, expandable -->
        <div v-else-if="b.kind === 'thought'" class="block thought">
          <div class="tool-line" @click="toggle(i)">
            <span class="tool-title">thinking</span>
            <span class="tool-summary">{{ (b.text ?? "").slice(0, 160) }}</span>
            <span class="chevron">{{ isCollapsed(i) ? "▸" : "▾" }}</span>
          </div>
          <div v-if="!isCollapsed(i)" class="body thought-body">
            {{ b.text }}
          </div>
        </div>
        <!-- Plans -->
        <div v-else-if="b.kind === 'plan'" class="block">
          <div class="who">plan</div>
          <div class="body">
            <div v-for="(entry, j) in b.entries" :key="j">
              [{{ entry[1] }}] {{ entry[0] }}
            </div>
          </div>
        </div>
        <!-- User / agent messages -->
        <div
          v-else-if="b.kind === 'user_message' || b.kind === 'agent_message'"
          class="block"
          :class="{
            user: b.kind === 'user_message',
            agent: b.kind === 'agent_message',
          }"
        >
          <div class="who">
            {{ b.kind === "user_message" ? "you" : session.agent_kind }}
          </div>
          <!-- Loom's orientation note (goal + "You are working in a Loom
               session…") is real prompt text the agent saw — keep it in the
               transcript, but collapse the boilerplate behind a disclosure
               so the goal leads. -->
          <template v-if="b.kind === 'user_message' && splitEntrance(b.text ?? '').entrance">
            <div v-if="splitEntrance(b.text ?? '').goal" class="body">
              {{ splitEntrance(b.text ?? "").goal }}
            </div>
            <div class="entrance">
              <div class="entrance-line" @click="toggle(i)">
                <span class="tool-summary">loom orientation</span>
                <span class="chevron">{{ isCollapsed(i) ? "▸" : "▾" }}</span>
              </div>
              <div v-if="!isCollapsed(i)" class="body entrance-body">
                {{ splitEntrance(b.text ?? "").entrance }}
              </div>
            </div>
          </template>
          <div v-else class="body">{{ b.text }}</div>
        </div>
        <!-- usage / turn_end / unknown: no visual block -->
      </template>
      <div v-if="blocks.length === 0" style="color: var(--text-dim)">
        No conversation yet.
      </div>
    </div>
    <div class="composer">
      <textarea
        v-model="draft"
        placeholder="Message the agent…"
        @keydown.enter.exact.prevent="send"
      ></textarea>
      <button class="primary" :disabled="!draft.trim() || busy" @click="send">
        Send
      </button>
    </div>
  </section>
</template>
