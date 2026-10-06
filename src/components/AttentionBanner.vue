<script setup lang="ts">
import { computed, ref, watch } from "vue";
import type { SessionSummary } from "../App.vue";
import { attentionNotices, retainActiveDismissals } from "../attentionNotifications";

const props = defineProps<{
  fleet: SessionSummary[];
  selectedId?: string | null;
  /** Reset local dismissals when changing Loom servers. */
  connectionKey?: string;
}>();
const emit = defineEmits<{
  (e: "select", sessionId: string): void;
  (e: "home"): void;
}>();
const hidden = ref(new Set<string>());
const notices = computed(() => attentionNotices(props.fleet));
watch(notices, (current) => { hidden.value = retainActiveDismissals(hidden.value, current); });
watch(() => props.connectionKey, () => { hidden.value = new Set(); });
const visible = computed(() => notices.value.filter((notice) =>
  notice.sessionId !== props.selectedId && !hidden.value.has(notice.key),
));
const current = computed(() => visible.value[0]);
function hide() {
  if (current.value) hidden.value = new Set([...hidden.value, current.value.key]);
}
</script>

<template>
  <aside v-if="current" class="attention-banner" :class="current.level" aria-label="Needs you">
    <div class="attention-banner-heading">
      <strong>Needs you<span v-if="visible.length > 1"> · {{ visible.length }}</span></strong>
      <button type="button" class="attention-banner-close" aria-label="Hide this notification"
        title="Hide this notification; the request stays in Needs You" @click="hide">×</button>
    </div>
    <div :key="current.key" role="status" aria-live="polite" aria-atomic="true">
      <div class="attention-banner-title" :title="current.title">{{ current.title }}</div>
      <p class="attention-banner-reason" :title="current.reason">{{ current.reason }}</p>
    </div>
    <div class="attention-banner-actions">
      <button type="button" class="primary" @click="emit('select', current.sessionId)">{{ current.action }}</button>
      <button v-if="current.topicId !== current.sessionId" type="button"
        @click="emit('select', current.topicId)">Coordinator</button>
      <button type="button" @click="emit('home')">View all</button>
    </div>
  </aside>
</template>

<style scoped>
.attention-banner {
  position: fixed; bottom: 16px; right: 16px; z-index: 45;
  width: min(340px, calc(100vw - 32px)); padding: 12px 14px;
  background: var(--bg-raised); border: 1px solid var(--border);
  border-left: 3px solid var(--attention); border-radius: 9px;
  box-shadow: 0 8px 30px #0006;
}
.attention-banner.blocked { border-left-color: var(--blocked); }
.attention-banner-heading { display: flex; align-items: center; justify-content: space-between; gap: 8px; }
.attention-banner-heading strong { color: var(--attention); font-size: 11px; text-transform: uppercase; letter-spacing: .05em; }
.attention-banner.blocked strong { color: var(--blocked); }
.attention-banner-close { background: transparent; border: 0; font-size: 18px; padding: 0 4px; }
.attention-banner-title { margin-top: 5px; font-weight: 600; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.attention-banner-reason { margin: 5px 0 12px; color: var(--text-dim); white-space: pre-wrap; overflow-wrap: anywhere; display: -webkit-box; -webkit-line-clamp: 3; -webkit-box-orient: vertical; overflow: hidden; user-select: text; }
.attention-banner-actions { display: flex; flex-wrap: wrap; gap: 6px; }
.attention-banner-actions button { padding: 5px 8px; font-size: 11px; }
</style>
