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

import { ref, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";

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

export const launchAgents = ref<AgentMetadata[]>([]);
export const launchProfiles = ref<ProfileLite[]>([]);
export const launchDefaultAgent = ref("");
export const launchSelection = ref<LaunchSelection>(loadSelection());
export const launchPresets = ref<LaunchPreset[]>(loadPresets());

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
export async function ensureLaunchConfig() {
  const cfg = await invoke<{
    agents: AgentMetadata[];
    profiles: ProfileLite[];
    default_agent: string;
  }>("launch_options");
  launchAgents.value = cfg.agents ?? [];
  launchProfiles.value = cfg.profiles ?? [];
  launchDefaultAgent.value = cfg.default_agent ?? "";
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
