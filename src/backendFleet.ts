import { normalizeServerUrl, readServerProfiles, rememberServer, type ServerProfile } from "./serverProfiles.ts";
import { scopeLayout, scopeSession } from "./backendIdentity.ts";
import type { PreferenceStorage } from "./launchDefaults.ts";
import type { SessionSummary, SessionLayout, LaunchOptions } from "./App.vue";

export const PROJECT_CACHE_KEY = "arachne.backendProjects.v1";
export interface BackendFleet extends ServerProfile {
  available: boolean;
  connecting: boolean;
  error: string | null;
  sessions: SessionSummary[];
  layout: SessionLayout | null;
  launchOptions: LaunchOptions | null;
}

/** Include the previous single server even if it predates the server list. */
export function registeredBackends(storage: PreferenceStorage, legacyUrl: string): BackendFleet[] {
  const legacy = normalizeServerUrl(legacyUrl);
  const profiles = readServerProfiles(storage);
  if (!profiles.some(profile => profile.url === legacy)) {
    const profile = { name: new URL(legacy).host, url: legacy };
    profiles.push(profile);
    // Remember the migrated backend before a different default can replace
    // loomUrl, so the original fleet remains registered on the next boot.
    rememberServer(storage, profile);
  }
  let cached: Record<string, SessionLayout> = {};
  try {
    const value = JSON.parse(storage.getItem(PROJECT_CACHE_KEY) || "{}");
    if (value && typeof value === "object" && !Array.isArray(value)) cached = value;
  } catch { /* Cold boot. */ }
  return profiles.map(profile => {
    const saved = cached[profile.url];
    let layout: SessionLayout | null = null;
    try { if (saved && Array.isArray(saved.spaces)) layout = scopeLayout(profile.url, saved); } catch { /* Invalid cached row. */ }
    return { ...profile, available: false, connecting: true, error: null, sessions: [], layout, launchOptions: null };
  });
}

export function acceptSnapshot(backend: BackendFleet, sessions: SessionSummary[], layout: SessionLayout): void {
  backend.sessions = sessions.map(session => scopeSession(backend.url, session));
  backend.layout = scopeLayout(backend.url, layout);
  backend.available = true;
  backend.connecting = false;
  backend.error = null;
}
export function markUnavailable(backend: BackendFleet, message: string): void {
  backend.available = false;
  backend.connecting = false;
  backend.error = message;
}

/** Cache headings/filing only, never chat contents, execution state, or credentials. */
export function cacheProjectLayout(storage: PreferenceStorage, url: string, layout: SessionLayout): void {
  try {
    const value = JSON.parse(storage.getItem(PROJECT_CACHE_KEY) || "{}");
    const cache = value && typeof value === "object" && !Array.isArray(value) ? value : {};
    cache[normalizeServerUrl(url)] = { ...layout, spaces: layout.spaces.map(space => ({
      ...space, groups: space.groups.map(group => ({ ...group, session_ids: [] })),
    })) };
    storage.setItem(PROJECT_CACHE_KEY, JSON.stringify(cache));
  } catch { /* Persistence is a convenience. */ }
}
export function registerBackend(storage: PreferenceStorage, profiles: BackendFleet[], profile: ServerProfile): BackendFleet {
  const url = normalizeServerUrl(profile.url);
  rememberServer(storage, { ...profile, url });
  const existing = profiles.find(backend => backend.url === url);
  if (existing) { existing.name = profile.name || existing.name; return existing; }
  const backend: BackendFleet = { name: profile.name || new URL(url).host, url, available: false, connecting: false,
    error: null, sessions: [], layout: null, launchOptions: null };
  profiles.push(backend);
  return backend;
}

export function combinedFleet(backends: BackendFleet[]): SessionSummary[] {
  return backends.filter(backend => backend.available).flatMap(backend => backend.sessions);
}
export function combinedLayout(backends: BackendFleet[]): SessionLayout {
  return { revision: 0, spaces: backends.flatMap(backend => backend.layout?.spaces ?? []),
    defaults: backends.flatMap(backend => backend.layout?.defaults ?? []) };
}
