import { invoke as tauriInvoke } from "@tauri-apps/api/core";
import { computed, hasInjectionContext, inject, toValue, type MaybeRefOrGetter } from "vue";
import { scopedId, splitScopedId, scopeSession, scopeLayout } from "./backendIdentity.ts";
import { normalizeServerUrl } from "./serverProfiles.ts";
import type { SessionLayout, SessionView } from "./App.vue";

export const BACKEND_URL_KEY = "backendUrl";
export function useBackendUrl() {
  const source = hasInjectionContext() ? inject<MaybeRefOrGetter<string | null | undefined>>(BACKEND_URL_KEY, undefined) : undefined;
  return computed(() => { const url = toValue(source); return url ? normalizeServerUrl(url) : null; });
}
const argumentIdentityKeys = new Set([
  "id", "session", "sessionId", "session_id", "sessionIds", "session_ids",
  "sourceId", "source_id", "destinationId", "destination_id", "parentId", "parent_id",
  "topicId", "topic_id", "topicIds", "topic_ids", "projectId", "project_id",
  "groupId", "group_id", "spaceId", "space_id", "resourceId", "resource_id",
  "destinationGroupId", "destination_group_id", "resourceIds", "resource_ids",
  "bindingId", "binding_id", "bindingIds", "binding_ids", "hiddenBindingKeys", "hidden_binding_keys",
  "todoId", "todo_id", "coordinatorId", "coordinator_id",
]);
export type BackendTransport = <T>(command: string, args: Record<string, unknown>) => Promise<T>;

/** Resolve ownership before touching the transport. No active/global server fallback. */
export function resolveBackendArgs(args: Record<string, unknown>, baseUrl?: string | null): { baseUrl: string; args: Record<string, unknown> } {
  const servers = new Set<string>();
  function strip(value: unknown, key = ""): unknown {
    if (typeof value === "string") {
      const identity = argumentIdentityKeys.has(key) ? splitScopedId(value) : null;
      if (identity) { servers.add(identity.baseUrl); return identity.id; }
      return value;
    }
    if (Array.isArray(value)) return value.map(item => strip(item, key));
    if (value && typeof value === "object") return Object.fromEntries(Object.entries(value).map(([key, val]) => [key, strip(val, key)]));
    return value;
  }
  const clean = strip(args) as Record<string, unknown>;
  if (args.baseUrl != null) servers.add(normalizeServerUrl(String(args.baseUrl)));
  if (baseUrl) servers.add(normalizeServerUrl(baseUrl));
  if (servers.size > 1) throw new Error("Arguments belong to different Loom servers.");
  const server = [...servers][0];
  if (!server) throw new Error("An explicit Loom backend URL is required.");
  return { baseUrl: server, args: { ...clean, baseUrl: server } };
}

const resourceCommands = new Set(["topic_resources", "read_topic_resource", "attach_topic_resource", "detach_topic_resource", "hide_topic_resource", "unhide_topic_resource", "project_bindings", "add_project_binding", "remove_project_binding"]);
const todoCommands = new Set(["user_todos", "topic_todos", "add_todo", "toggle_todo", "remove_todo"]);
const sessionCommands = new Set(["open_session", "launch_session", "delegate_task", "unarchive_session"]);
const layoutCommands = new Set(["session_layout", "create_group", "move_to_group", "delete_group"]);
const referenceKeys = new Set(["session_id", "parent_session_id", "source_session_id", "target_session_id", "child_session_id", "coordinator_id", "topic_id", "project_id", "group_id", "space_id", "resource_id", "binding_id", "channel_id", "message_id"]);

/** Scope only identity-bearing reply surfaces; model/config/chat block IDs stay wire values. */
export function scopeBackendResult<T>(command: string, baseUrl: string, result: T): T {
  if (result == null) return result;
  if (sessionCommands.has(command)) return scopeSession(baseUrl, result as unknown as SessionView) as T;
  if (layoutCommands.has(command) && typeof result === "object" && "spaces" in result) return scopeLayout(baseUrl, result as unknown as SessionLayout) as T;
  if (["review_session", "set_autoland"].includes(command)) {
    const reply = result as unknown as { session: SessionView };
    return { ...reply, session: scopeSession(baseUrl, reply.session) } as T;
  }
  const scopeIds = resourceCommands.has(command) || todoCommands.has(command) || command === "track_activity";
  if (!scopeIds && !["integrate_session", "land_thread", "land_topic", "publish_track", "send_to_thread"].includes(command)) return result;
  const identityRows = new Set(["resources", "hidden_resources", "resource", "project", "bindings", "todos", "channels", "messages", "handoffs", "watches"]);
  function scope(value: unknown, key = "", parent?: Record<string, unknown>, identityRow = false): unknown {
    if (typeof value === "string" && value && (referenceKeys.has(key) || (scopeIds && identityRow && key === "id") || (key === "author_id" && parent?.author_kind === "session"))) return scopedId(baseUrl, value);
    if (Array.isArray(value)) return value.map(item => scope(item, key));
    if (value && typeof value === "object") {
      const record = value as Record<string, unknown>;
      return Object.fromEntries(Object.entries(record).map(([k, v]) => [k, scope(v, k, record, identityRows.has(key))]));
    }
    return value;
  }
  return scope(result) as T;
}
export function createBackendInvoke(transport: BackendTransport) {
  return async function invoke<T>(command: string, args: Record<string, unknown> = {}, baseUrl?: string | null): Promise<T> {
    const resolved = resolveBackendArgs(args, baseUrl);
    return scopeBackendResult(command, resolved.baseUrl, await transport<T>(command, resolved.args));
  };
}
export const backendInvoke = createBackendInvoke(tauriInvoke);
export function useBackendInvoke(owner?: MaybeRefOrGetter<string | null | undefined>) {
  const injectedUrl = useBackendUrl();
  const backendUrl = computed(() => owner === undefined ? injectedUrl.value : toValue(owner));
  return <T>(command: string, args: Record<string, unknown> = {}, baseUrl?: string | null): Promise<T> => {
    // A session/resource owns its calls even when the operation composer points elsewhere.
    const hasOwner = (value: unknown, key = ""): boolean => typeof value === "string" ? argumentIdentityKeys.has(key) && !!splitScopedId(value)
      : Array.isArray(value) ? value.some(item => hasOwner(item, key))
      : !!value && typeof value === "object" && Object.entries(value).some(([key, val]) => hasOwner(val, key));
    return backendInvoke<T>(command, args, baseUrl ?? (hasOwner(args) || args.baseUrl ? undefined : backendUrl.value));
  };
}

export function partitionBackendIds(ids: string[]): Map<string, string[]> {
  const groups = new Map<string, string[]>();
  for (const id of ids) {
    const identity = splitScopedId(id);
    if (!identity) throw new Error("Aggregate operations require backend-qualified identities.");
    const group = groups.get(identity.baseUrl) ?? [];
    group.push(id); groups.set(identity.baseUrl, group);
  }
  return groups;
}
export function matchesChatEnvelope(sessionId: string, envelope: { server_url: string; session_id: string }): boolean {
  const identity = splitScopedId(sessionId);
  if (!identity) return false;
  try { return identity.baseUrl === normalizeServerUrl(envelope.server_url) && identity.id === envelope.session_id; }
  catch { return false; }
}
