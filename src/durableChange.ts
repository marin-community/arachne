import { scopedId, splitScopedId } from "./backendIdentity.ts";
import { isValidTopicBranch } from "./trackPolicy.ts";

export interface ChangeCompletion { channel_id: string; message_id: string }
export interface RangeScope { kind: "range"; base_oid: string; head_oid: string; paths: string[] }
export interface WorkingScope { kind: "working"; base_oid: string; head_oid: string; paths: string[]; worktree_version: string }
export type ChangeScope = RangeScope | WorkingScope;
export interface ChangeIdentity {
  version: 1;
  id: string;
  title: string;
  source_session: string;
  repo_root: string;
  source_branch: string;
  source_work_dir: string;
}
export type DurableChange = ChangeIdentity & (
  { scope: RangeScope; completion: ChangeCompletion } |
  { scope: WorkingScope; completion: null }
);
/** Own-channel final result declarations cannot reference their enclosing message. */
export type InlineChange = ChangeIdentity & { scope: ChangeScope; completion: null };
export interface ChangeAcceptance {
  version: 1;
  change_id: string;
  source_session: string;
  scope: RangeScope;
  target_session: string;
  target_branch: string;
  target_oid: string;
  outcome: "accepted";
  completion: ChangeCompletion;
}
export interface ChangeSource {
  id: string;
  work_dir: string;
  branch: { repo_root: string; branch: string; tags: { key: string; value: string }[] };
}
type RecordValue = Record<string, unknown>;
const record = (value: unknown): value is RecordValue => !!value && typeof value === "object" && !Array.isArray(value);
const text = (value: unknown): value is string => typeof value === "string" && !!value.trim() && !/[\x00-\x1f\x7f]/.test(value);
const keys = (value: RecordValue, expected: string[]) => Object.keys(value).length === expected.length && expected.every(key => Object.prototype.hasOwnProperty.call(value, key));
const id = (value: unknown): value is string => typeof value === "string" && /^[A-Za-z0-9._-]{1,80}$/.test(value);
const oid = (value: unknown): value is string => typeof value === "string" && /^(?:[a-fA-F0-9]{40}|[a-fA-F0-9]{64})$/.test(value);
const absolute = (value: unknown): value is string => text(value) && value.startsWith("/");
const path = (value: unknown): value is string => text(value) && !value.startsWith("/") && !value.includes("\\") && value.split("/").every(part => !!part && part !== "." && part !== ".." && part.toLowerCase() !== ".git");
function decode(value: unknown): unknown {
  if (typeof value !== "string") return value;
  try { return JSON.parse(value); } catch { return null; }
}
function completion(value: unknown): ChangeCompletion | null {
  if (!record(value) || !keys(value, ["channel_id", "message_id"]) || !text(value.channel_id) || !text(value.message_id)) return null;
  return { channel_id: value.channel_id, message_id: value.message_id };
}
function scope(value: unknown): ChangeScope | null {
  if (!record(value) || (value.kind !== "range" && value.kind !== "working")) return null;
  if (!keys(value, ["kind", "base_oid", "head_oid", "paths", ...(value.kind === "working" ? ["worktree_version"] : [])])
    || !oid(value.base_oid) || !oid(value.head_oid) || value.base_oid.length !== value.head_oid.length
    || !Array.isArray(value.paths) || !value.paths.every(path) || new Set(value.paths).size !== value.paths.length) return null;
  const common = { base_oid: value.base_oid, head_oid: value.head_oid, paths: [...value.paths] as string[] };
  if (value.kind === "range") return { kind: "range", ...common };
  if (!value.paths.length || !text(value.worktree_version)) return null;
  return { kind: "working", ...common, worktree_version: value.worktree_version };
}

/** Decode untrusted advisory metadata. This does not verify result provenance. */
export function parseChange(input: unknown): DurableChange | null;
export function parseChange(input: unknown, inline: true): InlineChange | null;
export function parseChange(input: unknown, inline = false): DurableChange | InlineChange | null {
  const value = decode(input);
  if (!record(value) || !keys(value, ["version", "id", "title", "source_session", "repo_root", "source_branch", "source_work_dir", "scope", "completion"])
    || value.version !== 1 || !id(value.id) || !text(value.title) || !text(value.source_session)
    || !absolute(value.repo_root) || !absolute(value.source_work_dir) || !text(value.source_branch) || !isValidTopicBranch(value.source_branch)) return null;
  const selectedScope = scope(value.scope);
  const result = value.completion === null ? null : completion(value.completion);
  if (!selectedScope || (value.completion !== null && !result)
    || (selectedScope.kind === "working" && result !== null)
    || (inline && result !== null) || (!inline && selectedScope.kind === "range" && !result)
    || (result && result.channel_id !== value.source_session)) return null;
  const identity: ChangeIdentity = { version: 1, id: value.id, title: value.title, source_session: value.source_session,
    repo_root: value.repo_root, source_branch: value.source_branch, source_work_dir: value.source_work_dir };
  if (inline) return { ...identity, scope: selectedScope, completion: null };
  if (selectedScope.kind === "working") return { ...identity, scope: selectedScope, completion: null };
  if (!result) return null;
  return { ...identity, scope: selectedScope, completion: result };
}
export function parseAcceptance(input: unknown): ChangeAcceptance | null {
  const value = decode(input);
  if (!record(value) || !keys(value, ["version", "change_id", "source_session", "scope", "target_session", "target_branch", "target_oid", "outcome", "completion"])
    || value.version !== 1 || value.outcome !== "accepted" || !id(value.change_id) || !text(value.source_session)
    || !text(value.target_session) || !text(value.target_branch) || !isValidTopicBranch(value.target_branch) || !oid(value.target_oid)) return null;
  const acceptedScope = scope(value.scope), result = completion(value.completion);
  if (!acceptedScope || acceptedScope.kind !== "range" || !result || result.channel_id !== value.source_session || value.target_oid.length !== acceptedScope.head_oid.length) return null;
  return { version: 1, change_id: value.change_id, source_session: value.source_session, scope: acceptedScope,
    target_session: value.target_session, target_branch: value.target_branch, target_oid: value.target_oid,
    outcome: "accepted", completion: result };
}
/** UI selection ownership is separate from the unchanged server-local declaration. */
export interface ChangeSelection { sourceId: string; change: DurableChange }
export const changeKey = (change: DurableChange, sourceId: string) => `${sourceId}/${change.id}`;
export const selectionKey = (selected: ChangeSelection) => changeKey(selected.change, selected.sourceId);
function localSourceId(sourceId: string): string {
  const owner = splitScopedId(sourceId);
  if (!owner && sourceId.startsWith("loom:")) throw new Error("Invalid qualified source identity.");
  return owner?.id ?? sourceId;
}
function scopeReference(sourceId: string, id: string): string {
  const owner = splitScopedId(sourceId);
  return owner ? scopedId(owner.baseUrl, id) : id;
}

export const sameChange = (a: DurableChange, b: DurableChange) => {
  const left = parseChange(a), right = parseChange(b);
  return !!left && !!right && JSON.stringify(left) === JSON.stringify(right);
};
export interface RegisteredChange {
  key: string;
  sourceId: string;
  change: DurableChange;
  completion: ChangeCompletion | null;
  acceptance: ChangeAcceptance | null;
  blocked: string;
}
export interface ChangeRegistry { entries: RegisteredChange[]; errors: string[] }

/** Duplicate indexing is idempotent; ambiguity and mismatched receipts block actions. */
export function changeRegistry(source: ChangeSource): ChangeRegistry {
  const entries: RegisteredChange[] = [], errors: string[] = [];
  const groups = new Map<string, string[]>();
  for (const tag of source.branch.tags) {
    if (!tag.key.startsWith("change:")) continue;
    const values = groups.get(tag.key) ?? [];
    values.push(tag.value); groups.set(tag.key, values);
  }
  for (const [tagKey, values] of groups) {
    const parsed = values.map(value => parseChange(value));
    const change = parsed.find((value): value is DurableChange => value !== null);
    if (!change || tagKey !== `change:${change.id}`) { errors.push(`Invalid declaration ${source.id}/${tagKey.slice(7)}.`); continue; }
    let blocked = "";
    if (parsed.some(value => !value || !sameChange(change, value))) blocked = "Conflicting duplicate declaration; reconcile metadata before selecting.";
    if (change.source_session !== localSourceId(source.id) || change.repo_root !== source.branch.repo_root
      || change.source_branch !== source.branch.branch || change.source_work_dir !== source.work_dir) blocked = "Original source identity does not match the declaration.";
    const receipts = source.branch.tags.filter(tag => tag.key === `change_acceptance:${change.id}`).map(tag => parseAcceptance(tag.value));
    const receipt = receipts[0] ?? null;
    if (receipts.length && (!receipt || receipts.some(value => !value || JSON.stringify(value) !== JSON.stringify(receipt))
      || receipt.change_id !== change.id || receipt.source_session !== change.source_session
      || JSON.stringify(receipt.scope) !== JSON.stringify(change.scope) || JSON.stringify(receipt.completion) !== JSON.stringify(change.completion))) {
      blocked = "Invalid or mismatched acceptance receipt; reconcile accepted history before retrying.";
    }
    const scopedCompletion = change.completion ? {
      channel_id: scopeReference(source.id, change.completion.channel_id),
      message_id: scopeReference(source.id, change.completion.message_id),
    } : null;
    // Receipts shown in shared UI state use the same backend namespace. The tag
    // and exact expectedChange above remain wire values for validation/dispatch.
    const acceptance = !blocked && receipt ? { ...receipt,
      source_session: scopeReference(source.id, receipt.source_session),
      target_session: scopeReference(source.id, receipt.target_session),
      completion: { channel_id: scopeReference(source.id, receipt.completion.channel_id),
        message_id: scopeReference(source.id, receipt.completion.message_id) },
    } : null;
    entries.push({ key: changeKey(change, source.id), sourceId: source.id, change, completion: scopedCompletion, acceptance, blocked });
  }
  for (const tag of source.branch.tags) {
    if (tag.key.startsWith("change_acceptance:") && !entries.some(entry => tag.key === `change_acceptance:${entry.change.id}`)) {
      errors.push(`Receipt ${source.id}/${tag.key.slice(18)} has no valid original declaration; reconcile history.`);
    }
  }
  // Native selection validates the entire source registry. Keep valid accepted
  // history visible, while preventing a sibling action from bypassing ambiguity.
  if (errors.length || entries.some(entry => entry.blocked)) {
    for (const entry of entries) {
      if (!entry.acceptance && !entry.blocked) entry.blocked = "Source registry contains invalid or conflicting metadata; reconcile before selecting.";
    }
  }
  return { entries, errors };
}

/** Clone the selected declaration so live metadata never silently substitutes scope. */
export function pinChange(change: DurableChange): DurableChange {
  const pinned = parseChange(change);
  if (!pinned) throw new Error("Invalid selected Change.");
  Object.freeze(pinned.scope.paths); Object.freeze(pinned.scope);
  if (pinned.completion) Object.freeze(pinned.completion);
  return Object.freeze(pinned);
}
export function pinSelection(sourceId: string, change: DurableChange): ChangeSelection {
  if (localSourceId(sourceId) !== change.source_session) throw new Error("The selected declaration belongs to another source.");
  return Object.freeze({ sourceId, change: pinChange(change) });
}
export function selectionProblem(selected: ChangeSelection, source: ChangeSource | undefined): string {
  if (!source) return "The selected source is unavailable. Select a current declaration.";
  if (source.id !== selected.sourceId) return "The selected Change belongs to another source or Loom backend.";
  const entry = changeRegistry(source).entries.find(entry => entry.key === selectionKey(selected));
  if (!entry || !sameChange(selected.change, entry.change)) return "The declaration changed or was removed. Reselect it for fresh review.";
  if (entry.blocked) return entry.blocked;
  if (entry.acceptance) return "This exact Change has already been accepted.";
  return "";
}
export function selectedChangePayload(selected: ChangeSelection) {
  const pinned = pinSelection(selected.sourceId, selected.change);
  return { sessionId: pinned.sourceId, changeId: pinned.change.id, expectedChange: pinned.change };
}
