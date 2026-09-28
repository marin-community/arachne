<script setup lang="ts">
import { computed } from "vue";
import type { AcpUsage } from "../App.vue";
import { formatTokens } from "../chatRows";

const props = withDefaults(defineProps<{
  sessionId: string;
  usage?: AcpUsage | null;
}>(), { usage: null });

const percent = computed(() => {
  if (!props.usage || props.usage.size <= 0) return null;
  return Math.round((props.usage.used / props.usage.size) * 100);
});
const barWidth = computed(() => `${Math.min(100, Math.max(0, percent.value ?? 0))}%`);
const cost = computed(() => {
  const value = props.usage?.cost;
  if (!value || !Number.isFinite(value.amount)) return null;
  try {
    return new Intl.NumberFormat(undefined, {
      style: "currency",
      currency: value.currency,
      maximumFractionDigits: 4,
    }).format(value.amount);
  } catch {
    return `${value.amount.toFixed(4)} ${value.currency}`;
  }
});
const contextTitle = computed(() => {
  if (!props.usage || percent.value === null) return "Context usage unavailable";
  const remaining = Math.max(0, props.usage.size - props.usage.used);
  return `${formatTokens(remaining)} tokens remain in the current context window`;
});
</script>

<template>
  <div class="context-telemetry" aria-label="Session telemetry">
    <div v-if="usage && percent !== null" class="context-chip" :title="contextTitle">
      <span class="context-label">Context</span>
      <span class="context-track" role="progressbar" :aria-valuenow="Math.min(100, Math.max(0, percent))" aria-valuemin="0" aria-valuemax="100" :aria-label="`${percent}% context used`">
        <span class="context-fill" :style="{ width: barWidth }"></span>
      </span>
      <strong>{{ percent }}%</strong>
      <span class="context-count">{{ formatTokens(usage.used) }}/{{ formatTokens(usage.size) }}</span>
    </div>
    <div class="session-metrics">
      <span class="session-id" :title="sessionId">⌘ {{ sessionId }}</span>
      <span v-if="cost" class="session-cost" :title="`Provider-reported cumulative session cost: ${cost}`">◉ {{ cost }}</span>
    </div>
  </div>
</template>

<style scoped>
.context-telemetry {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 8px 14px;
  min-width: 0;
  font-variant-numeric: tabular-nums;
}
.context-chip {
  display: inline-flex;
  align-items: center;
  gap: 8px;
  max-width: 100%;
  padding: 5px 10px;
  border: 1px solid var(--accent);
  border-radius: 999px;
  background: color-mix(in srgb, var(--accent) 20%, var(--bg-raised));
  color: var(--text);
  white-space: nowrap;
}
.context-label { font-weight: 600; }
.context-track {
  display: inline-flex;
  width: clamp(55px, 8vw, 125px);
  height: 9px;
  overflow: hidden;
  border-radius: 999px;
  background: color-mix(in srgb, var(--bg) 75%, var(--accent));
}
.context-fill { display: block; height: 100%; border-radius: inherit; background: var(--accent); }
.context-count { color: var(--text-dim); }
.session-metrics { display: inline-flex; flex-wrap: wrap; align-items: center; gap: 12px; color: var(--text-dim); }
.session-id { max-width: 220px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.session-cost { white-space: nowrap; }
</style>
