<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import type { SessionView } from "../App.vue";
import { effectiveAutoland, threadAutoland, trackAutoland, type AutolandPolicy, type PolicySession } from "../trackPolicy.ts";

const props = defineProps<{ session: PolicySession; track: PolicySession | null; coordinator?: boolean }>();
const emit = defineEmits<{ (e: "updated", session: SessionView): void; (e: "error", message: string): void }>();
const saved = ref<SessionView | null>(null);
const busy = ref(false);
watch(() => [props.session.id, props.session.branch], () => { saved.value = null; });
const current = computed(() => saved.value ?? props.session);
const policy = computed(() => props.coordinator ? trackAutoland(current.value) : threadAutoland(current.value));
const effective = computed(() => effectiveAutoland(current.value, props.coordinator ? current.value : props.track));
async function change(event: Event) {
  const value = (event.target as HTMLSelectElement).value as AutolandPolicy;
  const id = props.session.id;
  if (busy.value) return;
  busy.value = true;
  try {
    const next = await invoke<SessionView>("set_autoland", { sessionId: id, policy: value });
    if (props.session.id === id) saved.value = next;
    emit("updated", next);
  } catch (error: any) {
    emit("error", error?.message ?? String(error));
  } finally {
    busy.value = false;
    // Keep a rejected change from remaining selected in the native control.
    (event.target as HTMLSelectElement).value = policy.value;
  }
}
</script>

<template>
  <label class="autoland-control" title="Automatically accept complete successful intended results into the canonical branch">
    <span>Autoland</span>
    <select :value="policy" :disabled="busy || session.status === 'archived' || !track" :aria-label="coordinator ? 'Track autoland' : 'Thread autoland'" @change="change">
      <option v-if="!coordinator" value="inherit">Inherit Track</option>
      <option value="on">On</option>
      <option value="off">Off</option>
    </select>
    <small v-if="!coordinator">Effective: {{ effective }}</small>
  </label>
</template>

<style scoped>
.autoland-control { display: inline-flex; align-items: center; gap: 5px; font-size: 12px; flex-wrap: wrap; }
small { color: var(--text-dim); }
</style>
