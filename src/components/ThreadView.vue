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
const emit = defineEmits<{ (e: "error", msg: string): void }>();

const blocks = ref<DisplayBlock[]>([]);
const draft = ref("");
const busy = ref(false);
const convEl = ref<HTMLElement | null>(null);
const unlisteners: UnlistenFn[] = [];

function scrollToBottom() {
  nextTick(() => {
    if (convEl.value) convEl.value.scrollTop = convEl.value.scrollHeight;
  });
}

async function reload() {
  try {
    blocks.value = await invoke<DisplayBlock[]>("fetch_chat", { id: props.session.id });
    scrollToBottom();
  } catch (e: any) {
    emit("error", e?.message ?? String(e));
  }
}

onMounted(async () => {
  await reload();
  // Live updates: every chat/session event for the open session triggers a
  // journal re-fetch (the journal is the source of truth; deltas are an
  // optimization for later). `resync` semantics from loom's SSE make this the
  // correct, simple baseline.
  unlisteners.push(
    await listen("loom://chat-event", async (event) => {
      const frame = event.payload as { topic: string; event: string };
      if (frame.topic.startsWith("chat:") || frame.topic.startsWith("session:")) {
        await reload();
      }
    })
  );
  unlisteners.push(
    await listen("loom://fleet", () => {
      // Status/attention badges change; the view's own status refreshes via
      // re-open. Lightweight: re-fetch view occasionally.
    })
  );
});

onUnmounted(() => unlisteners.forEach((u) => u()));

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

function kindOf(b: DisplayBlock): string {
  return b.kind;
}
function payloadOf(b: DisplayBlock): any {
  return b;
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
    </div>
    <div class="conversation" ref="convEl">
      <div
        v-for="(b, i) in blocks"
        :key="i"
        class="block"
        :class="{
          user: b.kind === 'user_message',
          agent: b.kind === 'agent_message',
          thought: b.kind === 'thought',
          tool: b.kind === 'tool_call',
        }"
      >
        <div class="who" v-if="b.kind === 'user_message'">you</div>
        <div class="who" v-else-if="b.kind === 'agent_message'">{{ session.agent_kind }}</div>
        <div class="who" v-else-if="b.kind === 'thought'">thinking</div>
        <div class="who" v-else-if="b.kind === 'tool_call'">tool</div>
        <div class="body" v-if="b.kind === 'tool_call'">
          <span class="status" :class="{ running: b.status === 'running' }">
            {{ b.status }}
          </span>
          · {{ b.title }}
          <span v-if="b.summary" style="color: var(--text-dim)">
            — {{ b.summary.slice(0, 300) }}
          </span>
        </div>
        <div class="body" v-else-if="b.kind === 'plan'">
          <div v-for="(entry, j) in b.entries" :key="j">
            [{{ entry[1] }}] {{ entry[0] }}
          </div>
        </div>
        <div class="body" v-else-if="b.kind === 'usage'" v-show="false"></div>
        <div class="body" v-else-if="b.kind === 'turn_end'" v-show="false"></div>
        <div class="body" v-else>{{ b.text ?? b.payload ?? "" }}</div>
      </div>
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
