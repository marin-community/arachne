<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import type { SessionView } from "../App.vue";
import { effectiveAutoland, threadAutoland, trackAutoland, type AutolandPolicy, type PolicySession } from "../trackPolicy.ts";

const props = defineProps<{ session: PolicySession; track: PolicySession | null; coordinator?: boolean }>();
const emit = defineEmits<{ (e: "updated", session: SessionView): void; (e: "error", message: string): void }>();
const saved = ref<SessionView | null>(null);
const busy = ref(false);
const warning = ref("");
const policyTag = (session: PolicySession) => JSON.stringify(session.branch.tags.find(tag =>
  tag.key === (props.coordinator ? "track_autoland" : "thread_autoland")) ?? null);
let beforeSaveTag = "";
watch(() => [props.session.id, props.session.branch], () => {
  // A delayed pre-save fleet snapshot must not undo an acknowledged tag write.
  // Once the tag changes (including its durable revision), use live props.
  if (saved.value?.id === props.session.id && policyTag(props.session) === beforeSaveTag) return;
  saved.value = null;
});
watch(() => props.session.id, () => { warning.value = ""; });
const current = computed(() => saved.value ?? props.session);
const policy = computed(() => props.coordinator ? trackAutoland(current.value) : threadAutoland(current.value));
const effective = computed(() => effectiveAutoland(current.value, props.coordinator ? current.value : props.track));
async function change(event: Event) {
  const value = (event.target as HTMLSelectElement).value as AutolandPolicy;
  const id = props.session.id;
  if (busy.value) return;
  busy.value = true;
  beforeSaveTag = policyTag(props.session);
  warning.value = "";
  try {
    const result = await invoke<{ session: SessionView; warnings: string[] }>("set_autoland", { sessionId: id, policy: value });
    const returnedTag = result.session.branch.tags.find(tag =>
      tag.key === (props.coordinator ? "track_autoland" : "thread_autoland"));
    // An empty revision marks the backend's acknowledged-write fallback.
    // A live tag that changed during the request supersedes that fallback.
    const superseded = props.session.id === id && returnedTag?.set_at === "" && policyTag(props.session) !== beforeSaveTag;
    if (props.session.id === id) {
      saved.value = superseded ? null : result.session;
      warning.value = result.warnings.join(" ");
    }
    if (!superseded) emit("updated", result.session);
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
    <small v-if="warning" class="policy-warning" role="status">{{ warning }}</small>
  </label>
</template>

<style scoped>
.autoland-control { display: inline-flex; align-items: center; gap: 5px; font-size: 12px; flex-wrap: wrap; }
small { color: var(--text-dim); }
</style>
