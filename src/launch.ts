// Launch presets: harness / model family / model / effort selections shared
// by the topic launcher (sidebar) and the delegate box (thread view).
//
// The selection persists to localStorage on every change, so the last chosen
// config is the default the next time Arachne launches or delegates — the
// "don't make me re-pick every time" rule.
//
// The fields map onto loom's launch overrides (`sessions.launch`'s
// agent/model/effort). An empty field means "inherit the server-side
// default", matching loom's omitted-vs-blank distinction; the Rust command
// filters empty strings to `None` before they reach loom.

import { computed, ref, watch } from "vue";
import { backendInvoke, useBackendUrl } from "./backendInvoke.ts";
import { normalizeServerUrl } from "./serverProfiles.ts";

export interface AgentChoice {
  id: string;
  label: string;
}

// One harness (agent runtime) the picker offers — claude, codex, pi, …
export interface AgentMetadata {
  kind: string;
  label: string;
  models: AgentChoice[];
  efforts: AgentChoice[];
  accepts_raw_model: boolean;
}

// A named launch profile on the server (coarse policy defaults). Kept for
// future surfacing; presets currently carry only the runtime axes.
export interface ProfileLite {
  name: string;
  description: string;
  agent_kind: string;
  model: string;
  effort: string;
  class: string;
}

export interface LaunchSelection {
  /** Harness (agent kind). "" = server default. */
  agent: string;
  /** Model id within the harness. "" = harness default. */
  model: string;
  /** Reasoning effort. "" = harness default. */
  effort: string;
}

export interface LaunchPreset {
  name: string;
  agent: string;
  model: string;
  effort: string;
}

const SELECTION_KEY = "arachne.launchSelection";
const PRESETS_KEY = "arachne.launchPresets";

function loadSelection(): LaunchSelection {
  try {
    const raw = localStorage.getItem(SELECTION_KEY);
    if (raw) {
      const v = JSON.parse(raw);
      if (v && typeof v === "object") {
        return {
          agent: String(v.agent ?? ""),
          model: String(v.model ?? ""),
          effort: String(v.effort ?? ""),
        };
      }
    }
  } catch {
    // Corrupt payload: fall through to the empty (server-default) selection.
  }
  return { agent: "", model: "", effort: "" };
}

function loadPresets(): LaunchPreset[] {
  try {
    const raw = localStorage.getItem(PRESETS_KEY);
    if (raw) {
      const v = JSON.parse(raw);
      if (Array.isArray(v)) {
        return v.filter(
          (p) =>
            p &&
            typeof p === "object" &&
            typeof p.name === "string" &&
            p.name.trim() !== "",
        ).map((p: any) => ({
          name: String(p.name),
          agent: String(p.agent ?? ""),
          model: String(p.model ?? ""),
          effort: String(p.effort ?? ""),
        }));
      }
    }
  } catch {
    // Corrupt payload: start fresh rather than crash the picker.
  }
  return [];
}

// --- Store (module singleton: app-lifetime, shared by every picker) --------

interface LaunchConfig { agents: AgentMetadata[]; profiles: ProfileLite[]; default_agent: string }
const catalogs = ref<Record<string, LaunchConfig>>({});
const pending = new Map<string, Promise<void>>();
export function launchConfigFor(baseUrl: string | null | undefined): LaunchConfig | undefined {
  return baseUrl ? catalogs.value[normalizeServerUrl(baseUrl)] : undefined;
}
export function useLaunchConfig() {
  const baseUrl = useBackendUrl();
  const config = computed(() => launchConfigFor(baseUrl.value));
  return {
    launchAgents: computed(() => config.value?.agents ?? []),
    launchProfiles: computed(() => config.value?.profiles ?? []),
    launchDefaultAgent: computed(() => config.value?.default_agent ?? ""),
    ensureLaunchConfig: () => ensureLaunchConfig(baseUrl.value),
  };
}
export const launchSelection = ref<LaunchSelection>(loadSelection());
export const launchPresets = ref<LaunchPreset[]>(loadPresets());

const selections = ref<Record<string, LaunchSelection>>({});
let legacySelectionUrl = "http://127.0.0.1:7878";
try { legacySelectionUrl = normalizeServerUrl(localStorage.getItem("loomUrl") || legacySelectionUrl); } catch { /* Browser tests/storage. */ }
export function launchSelectionFor(baseUrl: string | null | undefined): LaunchSelection {
  if (!baseUrl) return { agent: "", model: "", effort: "" };
  const url = normalizeServerUrl(baseUrl);
  if (!selections.value[url]) {
    let saved: LaunchSelection | undefined;
    try {
      const value = JSON.parse(localStorage.getItem(`${SELECTION_KEY}@${url}`) || "null");
      if (value && [value.agent, value.model, value.effort].every(field => typeof field === "string")) saved = value;
    } catch { /* Use server defaults. */ }
    selections.value[url] = saved ?? (url === legacySelectionUrl ? { ...launchSelection.value } : { agent: "", model: "", effort: "" });
  }
  return selections.value[url];
}
export function saveLaunchSelection(baseUrl: string, selection: LaunchSelection): void {
  const url = normalizeServerUrl(baseUrl);
  selections.value[url] = { ...selection };
  try { localStorage.setItem(`${SELECTION_KEY}@${url}`, JSON.stringify(selection)); } catch { /* Memory remains usable. */ }
}
export function useLaunchSelection() {
  const baseUrl = useBackendUrl();
  return computed({
    get: () => launchSelectionFor(baseUrl.value),
    set: (selection: LaunchSelection) => { if (baseUrl.value) saveLaunchSelection(baseUrl.value, selection); },
  });
}

// Persist on change — this is what makes the last chosen config the default.
watch(
  launchSelection,
  (v) => localStorage.setItem(SELECTION_KEY, JSON.stringify(v)),
  { deep: true },
);
watch(
  launchPresets,
  (v) => localStorage.setItem(PRESETS_KEY, JSON.stringify(v)),
  { deep: true },
);

// Fetch the harness/profile surface from the Rust core (the existing
// `launch_options` command already aggregates agents.list + profiles.list).
// Safe to call on every connect; failures (not connected yet) leave the
// last-known list and the persisted selection in place.
export async function ensureLaunchConfig(baseUrl?: string | null, fetchOptions = backendInvoke) {
  if (!baseUrl) throw new Error("An explicit Loom backend URL is required for launch options.");
  const url = normalizeServerUrl(baseUrl);
  if (pending.has(url)) return pending.get(url);
  const request = fetchOptions<LaunchConfig>("launch_options", {}, url).then(cfg => {
    catalogs.value = { ...catalogs.value, [url]: {
      agents: cfg.agents ?? [], profiles: cfg.profiles ?? [], default_agent: cfg.default_agent ?? "",
    } };
  }).finally(() => { pending.delete(url); });
  pending.set(url, request);
  return request;
}

// Save (or overwrite) a preset by name.
export function saveLaunchPreset(name: string, sel: LaunchSelection) {
  const trimmed = name.trim();
  if (!trimmed) return;
  const preset: LaunchPreset = {
    name: trimmed,
    agent: sel.agent,
    model: sel.model,
    effort: sel.effort,
  };
  const i = launchPresets.value.findIndex((p) => p.name === trimmed);
  if (i === -1) launchPresets.value.push(preset);
  else launchPresets.value.splice(i, 1, preset);
}

// Remove a preset by name.
export function deleteLaunchPreset(name: string) {
  launchPresets.value = launchPresets.value.filter((p) => p.name !== name);
}
