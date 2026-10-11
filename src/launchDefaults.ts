import { backendOf, splitScopedId } from "./backendIdentity.ts";
import { normalizeServerUrl } from "./serverProfiles.ts";
/** Device-local launch conveniences. No credentials or execution state. */
export interface PreferenceStorage {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
}
export interface ProjectLaunchDefaults {
  repo: string;
  base: string;
}
export const RECENT_REPOS_KEY = "arachne.recentRepos";
function serverKey(storage: PreferenceStorage, key: string, baseUrl?: string | null): string {
  // Project IDs and server-side paths are only meaningful on their Loom server.
  const server = normalizeServerUrl(baseUrl || storage.getItem("loomUrl") || "http://127.0.0.1:7878");
  return `${key}@${server}`;
}

export function recentRepositories(storage: PreferenceStorage, baseUrl?: string | null): string[] {
  try {
    const value = JSON.parse(storage.getItem(serverKey(storage, RECENT_REPOS_KEY, baseUrl)) || "[]");
    return Array.isArray(value) ? value.filter((r): r is string => typeof r === "string" && !!r.trim()) : [];
  } catch { return []; }
}

export function rememberRepository(storage: PreferenceStorage, repo: string, baseUrl?: string | null): void {
  const value = repo.trim();
  if (!value) return;
  try { storage.setItem(serverKey(storage, RECENT_REPOS_KEY, baseUrl), JSON.stringify([...new Set([value, ...recentRepositories(storage, baseUrl)])].slice(0, 20))); }
  catch { /* Convenience only; storage can be unavailable. */ }
}

export function readProjectDefaults(storage: PreferenceStorage, projectId: string, baseUrl?: string | null): ProjectLaunchDefaults | null {
  try {
    const identity = splitScopedId(projectId);
    const url = backendOf(projectId) || baseUrl;
    const rawId = identity?.id || projectId;
    const value = JSON.parse(storage.getItem(serverKey(storage, `arachne.projectLaunch.${projectId}`, url)) || storage.getItem(serverKey(storage, `arachne.projectLaunch.${rawId}`, url)) || "null");
    return value && typeof value.repo === "string" && typeof value.base === "string"
      ? { repo: value.repo, base: value.base } : null;
  } catch { return null; }
}

export function saveProjectDefaults(storage: PreferenceStorage, projectId: string, defaults: ProjectLaunchDefaults, baseUrl?: string | null): void {
  try { storage.setItem(serverKey(storage, `arachne.projectLaunch.${projectId}`, backendOf(projectId) || baseUrl), JSON.stringify(defaults)); }
  catch { /* Convenience only. */ }
  rememberRepository(storage, defaults.repo, backendOf(projectId) || baseUrl);
}
