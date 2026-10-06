<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { SessionSummary } from "../App.vue";
import ChatMarkdown from "./ChatMarkdown.vue";
import { deliverySummary, watchTriggerKind, watchTriggerLabel, type TrackActivitySnapshot } from "../trackActivity";

const props = defineProps<{ topic: SessionSummary }>();
const emit = defineEmits<{ (e: "select", sessionId: string): void }>();
const snapshot = ref<TrackActivitySnapshot | null>(null);
const loading = ref(false);
const error = ref("");
const filter = ref<"all" | "results" | "errors">("all");
const updatedAt = ref("");
let disposed = false;
let generation = 0;
let dirty = false;
let unlisten: UnlistenFn | undefined;
let refreshTimer: ReturnType<typeof setTimeout> | undefined;

async function refresh() {
  if (disposed) return;
  if (loading.value) { dirty = true; return; }
  const request = ++generation;
  const topicId = props.topic.id;
  loading.value = true;
  error.value = "";
  try {
    const result = await invoke<TrackActivitySnapshot>("track_activity", { topicId });
    if (disposed || request !== generation || topicId !== props.topic.id) return;
    snapshot.value = result;
    updatedAt.value = new Date().toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  } catch (cause: any) {
    if (!disposed && request === generation) error.value = cause?.message ?? String(cause);
  } finally {
    if (!disposed) {
      loading.value = false;
      if (dirty) { dirty = false; scheduleRefresh(); }
    }
  }
}
function scheduleRefresh() {
  // Event-burst debounce only: no interval or autonomous polling.
  if (refreshTimer || disposed) return;
  refreshTimer = setTimeout(() => { refreshTimer = undefined; void refresh(); }, 400);
}
onMounted(async () => {
  try {
    const stop = await listen("loom://fleet", scheduleRefresh);
    if (disposed) stop(); else unlisten = stop;
  } catch (cause: any) {
    error.value = `Live updates unavailable: ${cause?.message ?? String(cause)}`;
  }
  void refresh();
});
onUnmounted(() => {
  disposed = true;
  generation++;
  unlisten?.();
  if (refreshTimer) clearTimeout(refreshTimer);
});
watch(() => props.topic.id, () => {
  generation++;
  snapshot.value = null;
  updatedAt.value = "";
  if (loading.value) dirty = true; else void refresh();
});
const messages = computed(() => (snapshot.value?.messages ?? []).filter((m) =>
  filter.value === "all" || (filter.value === "results" ? m.kind === "result" : deliverySummary(m) === "Delivery failed"),
));
const channelById = computed(() => new Map((snapshot.value?.channels ?? []).map((c) => [c.id, c])));
const watches = computed(() => [...(snapshot.value?.watches ?? [])].sort((a, b) =>
  Number(b.last_outcome === "error") - Number(a.last_outcome === "error") || Number(b.enabled) - Number(a.enabled),
));
function time(iso: string | null | undefined) {
  if (!iso) return "Never";
  const date = new Date(iso);
  return Number.isNaN(date.getTime()) ? iso : date.toLocaleString();
}
</script>

<template>
  <div class="track-activity">
    <div class="activity-toolbar">
      <strong>Track activity</strong>
      <button type="button" :disabled="loading" @click="refresh">{{ loading ? "Loading…" : "Refresh" }}</button>
    </div>
    <p class="activity-hint">Mailbox records show delivery, not whether an agent completed the work.</p>
    <p class="activity-hint">Updates with thread events. Refresh for watch changes or messages that do not wake a thread.<span v-if="updatedAt"> Checked {{ updatedAt }}.</span></p>
    <p v-if="error" role="alert" class="activity-error">{{ error }}</p>
    <p v-for="warning in snapshot?.warnings ?? []" :key="warning" class="activity-error">{{ warning }}</p>
    <p v-if="loading && !snapshot" class="activity-hint">Reading Loom activity…</p>
    <template v-if="snapshot">
      <section>
        <h3>Watches <span>{{ watches.length }}</span></h3>
        <p class="activity-hint">Fleet watches whose repository filters include this Track. Their own rules decide when to act.</p>
        <p v-if="!watches.length" class="activity-empty">No matching watches reported.</p>
        <details v-for="item in watches" :key="item.id" class="activity-card" :open="item.last_outcome === 'error'">
          <summary><span>{{ item.name }}</span><span class="activity-chip" :class="{ failed: item.last_outcome === 'error' }">{{ item.enabled ? watchTriggerKind(item) : "Disabled" }}</span></summary>
          <p>{{ watchTriggerLabel(item) }}</p>
          <p class="activity-hint">{{ item.scope?.repo || item.trigger?.repo || "All repositories" }}<span v-if="item.scope?.attention"> · Attention filter: {{ item.scope.attention }}</span></p>
          <p class="activity-hint">Last run: {{ time(item.last_run_at) }}<span v-if="item.last_outcome"> · {{ item.last_outcome }}</span></p>
          <p v-if="item.next_run_at || item.wake_at" class="activity-hint">Next timer: {{ time(item.wake_at || item.next_run_at) }}</p>
          <p v-if="item.latest_run?.summary">{{ item.latest_run.summary }}</p>
          <pre v-if="item.latest_run?.stderr" class="activity-error">{{ item.latest_run.stderr }}</pre>
        </details>
      </section>
      <section>
        <h3>Delivery subscriptions</h3>
        <p class="activity-hint">Track and worker channels, plus shared channels that deliver to them. Observation-only subscriptions are not exposed by Loom.</p>
        <p v-if="snapshot.channels_truncated" class="activity-hint">Showing the 20 channels with the most recent messages.</p>
        <p v-if="!snapshot.channels.length" class="activity-empty">No channels reported.</p>
        <details v-for="channel in snapshot.channels" :key="channel.id" class="activity-card">
          <summary><span>{{ channel.name }}</span><span class="activity-chip">{{ channel.kind }}</span></summary>
          <button v-if="channel.session_id" type="button" @click="emit('select', channel.session_id)">Open thread</button>
          <p v-if="!channel.bindings?.length" class="activity-hint">No delivery destinations reported.</p>
          <div v-for="binding in channel.bindings" :key="binding.id" class="activity-binding">
            <button v-if="binding.target_session_id" type="button" @click="emit('select', binding.target_session_id)">{{ binding.label }}</button>
            <span v-else>{{ binding.label }}</span>
            <small>{{ binding.kind }}</small>
          </div>
        </details>
      </section>
      <section>
        <h3>Recent mailbox</h3>
        <label class="activity-filter">Show <select v-model="filter"><option value="all">All messages</option><option value="results">Results</option><option value="errors">Delivery errors</option></select></label>
        <p class="activity-hint">Up to 60 recent entries across these channels. Read markers stay unchanged.</p>
        <p v-if="!messages.length" class="activity-empty">No {{ filter === 'all' ? 'messages' : filter === 'results' ? 'results' : 'delivery errors' }} reported.</p>
        <details v-for="message in messages" :key="message.id" class="activity-card" :open="deliverySummary(message) === 'Delivery failed'">
          <summary><span>{{ channelById.get(message.channel_id)?.name || message.channel_id }} · {{ message.kind }}</span><span class="activity-chip" :class="{ failed: deliverySummary(message) === 'Delivery failed' }">{{ deliverySummary(message) }}</span></summary>
          <p class="activity-hint">{{ time(message.created_at) }} · {{ message.author_kind }} {{ message.author_id }}</p>
          <ChatMarkdown :text="message.body" />
          <div v-for="delivery in message.deliveries ?? []" :key="delivery.binding_id" class="activity-delivery">
            <span>{{ delivery.binding_kind }} · {{ delivery.state }} · {{ delivery.attempts }} {{ delivery.attempts === 1 ? 'attempt' : 'attempts' }}</span>
            <button v-if="delivery.target_session_id" type="button" @click="emit('select', delivery.target_session_id)">Open recipient</button>
            <p v-if="delivery.last_error" class="activity-error">{{ delivery.last_error }}</p>
          </div>
        </details>
      </section>
    </template>
  </div>
</template>

<style scoped>
.track-activity { padding: 14px; overflow-wrap: anywhere; }
.activity-toolbar, .activity-card summary, .activity-binding { display: flex; align-items: center; justify-content: space-between; gap: 8px; }
.activity-toolbar button, .activity-card button { font-size: 11px; padding: 4px 7px; }
.activity-hint, .activity-empty { color: var(--text-dim); font-size: 11px; line-height: 1.5; }
.activity-error { color: var(--blocked); font-size: 12px; white-space: pre-wrap; }
section { margin-top: 20px; }
h3 { font-size: 12px; margin: 0 0 8px; }
h3 span { color: var(--text-dim); margin-left: 5px; }
.activity-card { border: 1px solid var(--border); border-radius: 6px; padding: 9px; margin-top: 7px; font-size: 12px; }
.activity-card summary { cursor: pointer; align-items: flex-start; font-size: 11px; }
.activity-card summary > span:first-child { flex: 1; }
.activity-card summary::before { content: '▸'; color: var(--text-dim); }
.activity-card[open] summary::before { content: '▾'; }
.activity-card p { margin: 8px 0; }
.activity-chip { color: var(--accent); font-size: 10px; flex-shrink: 0; max-width: 45%; }
.activity-chip.failed { color: var(--blocked); }
.activity-binding { margin-top: 8px; }
.activity-binding small { color: var(--text-dim); }
.activity-filter { display: flex; align-items: center; gap: 8px; color: var(--text-dim); font-size: 11px; }
.activity-filter select { flex: 1; min-width: 0; }
.activity-delivery { border-top: 1px solid var(--border); margin-top: 10px; padding-top: 8px; font-size: 11px; }
.activity-delivery button { margin-left: 5px; }
</style>
