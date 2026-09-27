<script setup lang="ts">
import { ref, computed, onMounted, onUnmounted, nextTick, useId } from "vue";
import {
  launchAgents,
  launchDefaultAgent,
  launchSelection,
  launchPresets,
  ensureLaunchConfig,
  saveLaunchPreset,
  deleteLaunchPreset,
  type LaunchSelection,
} from "../launch";

// The launch-preset picker: harness / model family / model / effort, plus
// named presets. Sits inline as a compact button showing the current
// selection ("auto", or "claude · sonnet · high"); the popover exposes the
// four selects and preset save/apply/delete.

defineProps<{}>();

const emit = defineEmits<{
  /** The selection changed (also written through the shared store). */
  (e: "change", sel: LaunchSelection): void;
}>();

const open = ref(false);
const root = ref<HTMLElement | null>(null);
const presetName = ref("");
const nameInput = ref<HTMLInputElement | null>(null);
const uid = useId();

function onClickOutside(e: MouseEvent) {
  if (open.value && root.value && !root.value.contains(e.target as Node)) {
    open.value = false;
    presetName.value = "";
  }
}
function onKeydown(e: KeyboardEvent) {
  if (e.key === "Escape" && open.value) {
    open.value = false;
    presetName.value = "";
  }
}
onMounted(() => {
  document.addEventListener("mousedown", onClickOutside);
  document.addEventListener("keydown", onKeydown);
});
onUnmounted(() => {
  document.removeEventListener("mousedown", onClickOutside);
  document.removeEventListener("keydown", onKeydown);
});

function set(field: keyof LaunchSelection, value: string) {
  const next: LaunchSelection = { ...launchSelection.value, [field]: value };
  if (field === "agent") {
    // A new harness invalidates the model/effort ids (they're per-harness).
    next.model = "";
    next.effort = "";
  }
  launchSelection.value = next;
  emit("change", next);
}

// --- Derived choices -------------------------------------------------------

const agents = computed(() => launchAgents.value);
/// The effective harness: the selected one, else null (server default).
const agent = computed(
  () => launchAgents.value.find((a) => a.kind === launchSelection.value.agent) ?? null,
);
const agentDefaultHint = computed(() => {
  if (launchSelection.value.agent) return "";
  const def = launchDefaultAgent.value;
  return def ? ` (${def})` : "";
});

// Model family: a coarse grouping of the harness's models.
// - Structured ids (gpt-6-astra, gpt-5.6-sol) group by everything but the
//   final segment: gpt-6 [astra], gpt-5.6 [sol, terra, luna].
// - Two-segment ids (gpt-5.5) are their own family — grouping them by the
//   bare prefix would collapse distinct generations into a `gpt` bucket.
// - Flat lists (claude: haiku, sonnet, opus) have no natural families, so the
//   harness itself is the single family — the drill-down stays meaningful
//   instead of showing two identical selects.
interface Family {
  id: string;
  models: { id: string; label: string }[];
}
const families = computed<Family[]>(() => {
  const m = agent.value?.models ?? [];
  if (m.length === 0) return [];
  const structured = m.some((c) => c.id.split("-").length >= 3);
  if (!structured) {
    return [{ id: agent.value!.label, models: m }];
  }
  const byFam = new Map<string, Family>();
  for (const choice of m) {
    const parts = choice.id.split("-");
    const fam = parts.length >= 3 ? parts.slice(0, -1).join("-") : choice.id;
    if (!byFam.has(fam)) byFam.set(fam, { id: fam, models: [] });
    byFam.get(fam)!.models.push(choice);
  }
  return [...byFam.values()];
});

/// The family containing the selected model; "" = default (unset).
const selectedFamily = computed(() => {
  const id = launchSelection.value.model;
  if (!id) return "";
  return families.value.find((f) => f.models.some((mo) => mo.id === id))?.id ?? "";
});

function setFamily(fam: string) {
  // Choosing a family picks its first model; "" resets to harness default.
  set(
    "model",
    fam ? (families.value.find((f) => f.id === fam)?.models[0]?.id ?? "") : "",
  );
}

const familyModels = computed(
  () => families.value.find((f) => f.id === selectedFamily.value)?.models ?? [],
);

const efforts = computed(() => agent.value?.efforts ?? []);

/// Compact label for the closed button: "auto" when nothing is set.
const label = computed(() => {
  const s = launchSelection.value;
  if (!s.agent && !s.model && !s.effort) return "auto";
  const parts: string[] = [];
  if (s.agent)
    parts.push(agent.value?.label ?? s.agent);
  if (s.model) parts.push(shortModel(s.model));
  if (s.effort) parts.push(s.effort);
  return parts.join(" · ");
});

function shortModel(id: string): string {
  const m = agent.value?.models.find((c) => c.id === id);
  if (m) return m.label;
  // Raw models (accepts_raw_model) — show the id as-is.
  return id;
}

const rawModelAllowed = computed(() => agent.value?.accepts_raw_model ?? false);

// --- Presets ---------------------------------------------------------------

function applyPreset(name: string) {
  const p = launchPresets.value.find((x) => x.name === name);
  if (!p) return;
  launchSelection.value = { agent: p.agent, model: p.model, effort: p.effort };
  emit("change", launchSelection.value);
}

function saveCurrentAsPreset() {
  const name = presetName.value.trim();
  if (!name) return;
  saveLaunchPreset(name, launchSelection.value);
  presetName.value = "";
  nextTick(() => nameInput.value?.focus());
}

// Load the agent list lazily the first time the popover opens (and it's
// cheap to refetch — a stale custom-agent list is the only drift risk).
let loaded = false;
function toggleOpen() {
  open.value = !open.value;
  if (open.value && !loaded) {
    loaded = true;
    ensureLaunchConfig().catch(() => {
      // Leave lists empty; the picker degrades to raw text entry.
      loaded = false;
    });
  }
}
</script>

<template>
  <div class="lp-root" ref="root">
    <button
      class="lp-toggle"
      :title="'launch preset — harness / model family / model / effort'"
      @click="toggleOpen"
    >
      <span class="lp-gear" aria-hidden="true">⚙</span>
      <span class="lp-label">{{ label }}</span>
    </button>
    <div v-if="open" class="lp-popover">
      <div class="lp-grid">
        <label class="lp-field">
          <span class="lp-field-name">Harness</span>
          <select :value="launchSelection.agent" @change="set('agent', ($event.target as HTMLSelectElement).value)">
            <option value="">
              default{{ agentDefaultHint }}
            </option>
            <option v-for="a in agents" :key="a.kind" :value="a.kind">{{ a.label }}</option>
          </select>
        </label>
        <label class="lp-field">
          <span class="lp-field-name">Model family</span>
          <select :value="selectedFamily" @change="setFamily(($event.target as HTMLSelectElement).value)">
            <option value="">default</option>
            <option v-for="f in families" :key="f.id" :value="f.id">
              {{ f.id }}
            </option>
          </select>
        </label>
        <label class="lp-field">
          <span class="lp-field-name">Model</span>
          <!-- Raw-model harnesses (e.g. claude accepts any id): a datalist
               offers the choices while still allowing free text. -->
          <input
            v-if="rawModelAllowed"
            :value="launchSelection.model"
            :list="`${uid}-models`"
            placeholder="default"
            spellcheck="false"
            @input="set('model', ($event.target as HTMLInputElement).value)"
          />
          <select
            v-else
            :value="launchSelection.model"
            @change="set('model', ($event.target as HTMLSelectElement).value)"
          >
            <option value="">default</option>
            <option v-for="m in familyModels" :key="m.id" :value="m.id">{{ m.label }}</option>
          </select>
          <datalist :id="`${uid}-models`">
            <option v-for="m in agent?.models ?? []" :key="m.id" :value="m.id">{{ m.label }}</option>
          </datalist>
        </label>
        <label class="lp-field">
          <span class="lp-field-name">Effort</span>
          <select :value="launchSelection.effort" @change="set('effort', ($event.target as HTMLSelectElement).value)">
            <option value="">default</option>
            <option v-for="e in efforts" :key="e.id" :value="e.id">{{ e.label }}</option>
          </select>
        </label>
      </div>

      <div class="lp-presets">
        <div class="lp-field-name">Presets</div>
        <div v-if="launchPresets.length === 0" class="lp-hint">
          No presets saved yet — dial in a config below and name it.
        </div>
        <div v-for="p in launchPresets" :key="p.name" class="lp-preset-row">
          <button class="lp-preset-apply" :title="`${p.agent || 'default'} · ${p.model || 'default model'} · ${p.effort || 'default effort'}`" @click="applyPreset(p.name)">
            {{ p.name }}
          </button>
          <button class="lp-preset-del" title="delete preset" @click="deleteLaunchPreset(p.name)">✕</button>
        </div>
        <div class="lp-save-row">
          <input
            ref="nameInput"
            v-model="presetName"
            placeholder="preset name…"
            spellcheck="false"
            @keydown.enter.prevent="saveCurrentAsPreset"
          />
          <button class="primary" :disabled="!presetName.trim()" @click="saveCurrentAsPreset">Save</button>
        </div>
      </div>
      <div class="lp-hint">
        Applies to every new topic and delegation; the last choice is
        remembered. Empty fields inherit the server's defaults.
      </div>
    </div>
  </div>
</template>
