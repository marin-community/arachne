<script setup lang="ts">
import { ref, onMounted, onUnmounted, nextTick } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { SessionView } from "../App.vue";

// DisplayBlock from src-tauri/src/blocks.rs, serialized internally-tagged:
// { kind: "user_message", text: "…", by: … } — flat fields keyed by `kind`.
interface DisplayBlock {
  kind: "user_message" | "agent_message" | "thought" | "tool_call" | "plan" | "usage" | "turn_end" | "other";
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

const props = defineProps<{ session: SessionView }>();
const emit = defineEmits<{
  (e: "error", msg: string): void;
  (e: "archive", id: string): void;
}>();

const blocks = ref<DisplayBlock[]>([]);
const draft = ref("");
const busy = ref(false);
const convEl = ref<HTMLElement | null>(null);
const unlisteners: UnlistenFn[] = [];

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
    blocks.value = await invoke<DisplayBlock[]>("fetch_chat", { id: props.session.id });
    if (stick) scrollToBottom();
  } catch (e: any) {
    emit("error", e?.message ?? String(e));
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
      if (frame.topic.startsWith("chat:") || frame.topic.startsWith("session:")) {
        scheduleReload();
      }
    })
  );
});

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
</script>

<template>
  <section class="main">
    <div class="thread-header">
      <div class="meta">
        <div class="name">{{ session.branch.name || session.id }}</div>
        <div class="sub">
          {{ session.agent_kind }} · {{ session.model || "auto" }} · turn {{ session.turn_count }} ·
          {{ session.work_dir }}
        </div>
      </div>
      <button @click="openInZed">Open in Zed</button>
      <button @click="interrupt">Interrupt</button>
      <button class="danger" @click="archive">Archive</button>
    </div>
    <div class="conversation" ref="convEl">
      <template v-for="(b, i) in blocks" :key="i">
        <!-- Tool calls: collapsed one-liner by default, expandable -->
        <div v-if="b.kind === 'tool_call'" class="block tool">
          <div class="tool-line" @click="toggle(i)">
            <span class="status" :class="{ running: b.status === 'running' }">{{ b.status }}</span>
            <span class="tool-title">{{ b.title }}</span>
            <span v-if="b.summary" class="tool-summary">{{ b.summary.slice(0, 200) }}</span>
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
          <div v-if="!isCollapsed(i)" class="body thought-body">{{ b.text }}</div>
        </div>
        <!-- Plans -->
        <div v-else-if="b.kind === 'plan'" class="block">
          <div class="who">plan</div>
          <div class="body">
            <div v-for="(entry, j) in b.entries" :key="j">[{{ entry[1] }}] {{ entry[0] }}</div>
          </div>
        </div>
        <!-- User / agent messages -->
        <div
          v-else-if="b.kind === 'user_message' || b.kind === 'agent_message'"
          class="block"
          :class="{ user: b.kind === 'user_message', agent: b.kind === 'agent_message' }"
        >
          <div class="who">{{ b.kind === "user_message" ? "you" : session.agent_kind }}</div>
          <div class="body">{{ b.text }}</div>
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
      <button class="primary" :disabled="!draft.trim() || busy" @click="send">Send</button>
    </div>
  </section>
</template>
