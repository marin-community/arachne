import type { PreferenceStorage } from "./launchDefaults";

export interface ServerProfile { name: string; url: string }
export const SERVER_PROFILES_KEY = "arachne.loomServers";

/** Match backend keychain URL identity; never accept credentials in a URL. */
export function normalizeServerUrl(raw: string): string {
  const url = new URL(raw.trim());
  if (!["http:", "https:"].includes(url.protocol) || !url.hostname || url.username || url.password || url.search || url.hash) {
    throw new Error("Use an HTTP(S) server URL without credentials, query, or fragment.");
  }
  return url.toString().replace(/\/+$/, "");
}

export function readServerProfiles(storage: PreferenceStorage): ServerProfile[] {
  try {
    const rows = JSON.parse(storage.getItem(SERVER_PROFILES_KEY) || "[]");
    if (!Array.isArray(rows)) return [];
    const result: ServerProfile[] = [];
    for (const row of rows) {
      if (typeof row?.name !== "string" || typeof row?.url !== "string") continue;
      try {
        const url = normalizeServerUrl(row.url);
        if (!result.some((r) => r.url === url)) result.push({ name: row.name.trim() || new URL(url).host, url });
      } catch { /* Invalid old entries are ignored. */ }
    }
    return result.slice(0, 12);
  } catch { return []; }
}

export function rememberServer(storage: PreferenceStorage, profile: ServerProfile): ServerProfile[] {
  const url = normalizeServerUrl(profile.url);
  const profiles = [{ name: profile.name.trim() || new URL(url).host, url }, ...readServerProfiles(storage).filter((p) => p.url !== url)].slice(0, 12);
  try { storage.setItem(SERVER_PROFILES_KEY, JSON.stringify(profiles)); }
  catch { /* Credentials remain solely in Keychain. */ }
  return profiles;
}
