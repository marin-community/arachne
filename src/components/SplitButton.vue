<script setup lang="ts">
// IntegrateSplitButton — the [ Integrate ▼ ] / [ Land ▼ ] split button
// (spec: docs/integration-and-landing.md, "Integrate UI" / "Land UI").
//
// Primary click runs the remembered/default strategy immediately; the
// dropdown exposes applicable strategies. Strategy memory is per-repo
// (spec: "Do not use one global integration preference for all
// repositories") — precedence: last-used for this repo, else the
// conservative fallback `Ask coordinator to decide`.

import { ref, computed, onMounted, onUnmounted, watch } from "vue";

export interface StrategyOption {
  value: string;
  label: string;
}

const props = defineProps<{
  /** "integrate" or "land" — the primary label and storage key. */
  kind: "integrate" | "land";
  /** Repo slug for per-repo strategy memory (owner/name). */
  repo: string;
  /** Offered strategies; only those that make sense for this worker. */
  options: StrategyOption[];
  /** Primary label; defaults to the capitalized kind. */
  label?: string;
  disabled?: boolean;
  busy?: boolean;
}>();

const emit = defineEmits<{
  (e: "run", strategy: string): void;
}>();

const open = ref(false);
const rootEl = ref<HTMLElement | null>(null);

const memoryKey = computed(
  () => `arachne:strategy:${props.kind}:${props.repo}`,
);

const selected = ref("ask");

function loadMemory() {
  // Per-repo last-used strategy; fall back to `ask` (conservative:
  // "Ask coordinator to decide") when nothing is remembered.
  const stored = localStorage.getItem(memoryKey.value);
  selected.value =
    stored && props.options.some((o) => o.value === stored) ? stored : "ask";
}

function onDocClick(e: MouseEvent) {
  if (rootEl.value && !rootEl.value.contains(e.target as Node)) {
    open.value = false;
  }
}

onMounted(() => {
  loadMemory();
  document.addEventListener("mousedown", onDocClick);
});

onUnmounted(() => document.removeEventListener("mousedown", onDocClick));

watch([memoryKey, () => props.options], loadMemory);

function primary() {
  if (props.disabled || props.busy) return;
  emit("run", selected.value);
}

function choose(value: string) {
  selected.value = value;
  open.value = false;
  emit("run", value);
}

const selectedLabel = computed(
  () =>
    props.options.find((o) => o.value === selected.value)?.label ??
    props.options[0]?.label ??
    "Ask coordinator to decide",
);
</script>

<template>
  <div class="split-button" ref="rootEl">
    <button
      class="split-primary"
      :class="{ busy: props.busy }"
      :disabled="props.disabled || props.busy"
      :title="`${selectedLabel} (remembered for ${props.repo})`"
      @click="primary()"
    >
      {{ props.busy ? "…" : `${props.label ?? (props.kind === "land" ? "Land" : "Integrate")}` }}
    </button>
    <button
      class="split-caret"
      :disabled="props.disabled || props.busy"
      :aria-label="`Choose ${props.kind} strategy`"
      title="Choose strategy"
      @click.stop="open = !open"
    >
      ▾
    </button>
    <div v-if="open" class="split-menu">
      <button
        v-for="o in props.options"
        :key="o.value"
        class="split-option"
        :class="{ current: o.value === selected }"
        type="button"
        @click="choose(o.value)"
      >
        <span class="check">{{ o.value === selected ? "✓" : "" }}</span>
        <span>{{ o.label }}</span>
      </button>
    </div>
  </div>
</template>

<style scoped>
.split-button {
  position: relative;
  display: inline-flex;
  align-items: stretch;
}
.split-primary {
  border: 1px solid var(--border, #3c3c46);
  border-right: none;
  border-radius: 6px 0 0 6px;
  background: var(--bg-raised, #26262e);
  color: var(--text, #eee);
  padding: 4px 10px;
  font-size: 12px;
  cursor: pointer;
}
.split-primary:not(:disabled):hover {
  background: var(--bg-hover, #32323c);
}
.split-caret {
  border: 1px solid var(--border, #3c3c46);
  border-radius: 0 6px 6px 0;
  background: var(--bg-raised, #26262e);
  color: var(--text-dim, #999);
  padding: 4px 6px;
  font-size: 10px;
  cursor: pointer;
}
.split-caret:not(:disabled):hover {
  background: var(--bg-hover, #32323c);
}
.split-primary:disabled,
.split-caret:disabled {
  opacity: 0.5;
  cursor: default;
}
.split-menu {
  position: absolute;
  top: calc(100% + 4px);
  right: 0;
  min-width: 220px;
  background: var(--bg-raised, #26262e);
  border: 1px solid var(--border, #3c3c46);
  border-radius: 6px;
  box-shadow: 0 8px 24px rgba(0, 0, 0, 0.4);
  z-index: 30;
  overflow: hidden;
}
.split-option {
  display: flex;
  gap: 8px;
  align-items: center;
  padding: 7px 10px;
  font-size: 12px;
  cursor: pointer;
  white-space: nowrap;
  width: 100%;
  border: 0;
  border-radius: 0;
  background: transparent;
  color: var(--text, #eee);
  text-align: left;
}
.split-option:hover {
  background: var(--bg-hover, #32323c);
}
.split-option.current {
  color: var(--accent, #7aa2f7);
}
.check {
  width: 12px;
  display: inline-block;
}
</style>
