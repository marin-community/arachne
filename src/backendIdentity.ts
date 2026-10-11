import { normalizeServerUrl } from "./serverProfiles.ts";
import type { SessionLayout, SessionSummary, SessionView } from "./App.vue";

/** UI identities are backend-qualified. Wire identities remain server-local. */
export function scopedId(baseUrl: string, id: string): string {
  return `loom:${encodeURIComponent(normalizeServerUrl(baseUrl))}:${encodeURIComponent(id)}`;
}

export function splitScopedId(value: string): { baseUrl: string; id: string } | null {
  if (!value.startsWith("loom:")) return null;
  const parts = value.slice(5).split(":");
  if (parts.length !== 2) return null;
  try { return { baseUrl: normalizeServerUrl(decodeURIComponent(parts[0])), id: decodeURIComponent(parts[1]) }; }
  catch { return null; }
}
export function backendOf(id: string | null | undefined): string | null {
  return id ? splitScopedId(id)?.baseUrl ?? null : null;
}

export function scopeSession<T extends SessionSummary | SessionView>(baseUrl: string, session: T): T {
  const id = (value: string) => scopedId(baseUrl, value);
  const nullable = (value: string | null | undefined) => value ? id(value) : value;
  return {
    ...session, id: id(session.id),
    ...("parent_id" in session ? { parent_id: nullable(session.parent_id), parent_session_id: nullable(session.parent_session_id) } : {}),
    branch: { ...session.branch, id: id(session.branch.id) },
    placement: session.placement ? {
      ...session.placement, space_id: nullable(session.placement.space_id),
      group_id: nullable(session.placement.group_id), session_id: nullable(session.placement.session_id),
    } : null,
  } as T;
}

export function scopeLayout(baseUrl: string, layout: SessionLayout): SessionLayout {
  const id = (value: string) => scopedId(baseUrl, value);
  return { ...layout, spaces: layout.spaces.map(space => ({
    ...space, id: id(space.id), groups: space.groups.map(group => ({
      ...group, id: id(group.id), space_id: id(group.space_id), session_ids: group.session_ids.map(id),
    })),
  })), defaults: layout.defaults.map(row => ({ ...row, group_id: id(row.group_id) })) };
}
