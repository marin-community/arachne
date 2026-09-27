// Persistent draft for the New-topic sheet.
//
// Selecting a topic while the new-topic sheet is open closes the sheet —
// the topic you clicked is the main pane's next content, and the sheet
// blocks it (both use grid-area main). But the half-written topic
// shouldn't die with the view: the draft is snapshotted to localStorage
// before close and restored on reopen, so reopening the sheet resumes
// exactly where you left off. Launching clears it — a launched topic
// exists as a real branch now.
//
// Everything here is pure and free of .vue imports so it runs under plain
// `node --test` with no build step (tests/newTopicDraft.test.mjs),
// mirroring src/topic-view.ts.

import type { FileAttachment } from "./attachments";

/** localStorage key holding the new-topic draft. */
export const NEW_TOPIC_DRAFT_KEY = "arachne.newTopicDraft";

/** Default repo, same default the sheet ships with. */
export const DEFAULT_REPO = "marin-community/arachne";

/** The draft shape the New-topic sheet edits in place. */
export interface NewTopicDraft {
  /** Optional card label; empty until typed. */
  title: string;
  /** The body: the agent's opening message and durable topic description. */
  body: string;
  /** owner/name to fork. */
  repo: string;
  /** Optional base branch; empty forks the repo's default. */
  base: string;
  /** Launch config (profile/agent/model/effort) — "" means "runtime default". */
  profile: string;
  agent: string;
  model: string;
  effort: string;
  /** @-mentions picked from the menu: token + stable resource ids. */
  mentions: { token: string; topicId: string; resourceId: string }[];
  /** Files (Scratch uploads): FileAttachment snapshots, base64 payloads and all. */
  attachments: FileAttachment[];
}

/** A fresh draft — the exact field defaults the sheet starts with. */
export function emptyDraft(): NewTopicDraft {
  return {
    title: "",
    body: "",
    repo: DEFAULT_REPO,
    base: "",
    profile: "default",
    agent: "",
    model: "",
    effort: "",
    mentions: [],
    attachments: [],
  };
}

/** The slice of localStorage this module needs — trivial to stub in tests. */
export interface DraftStorage {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
  removeItem(key: string): void;
}

function str(value: unknown, fallback = ""): string {
  return typeof value === "string" ? value : fallback;
}

/**
 * Read the draft, tolerating anything (absent key, corrupt JSON, a payload
 * from an older build): every field falls back to the empty-draft default,
 * so a bad snapshot reopens a blank-but-usable composer instead of a crash.
 */
export function readDraft(storage: DraftStorage): NewTopicDraft {
  const empty = emptyDraft();
  let raw: string | null = null;
  try {
    raw = storage.getItem(NEW_TOPIC_DRAFT_KEY);
  } catch {
    return empty; // storage unavailable — same as no draft
  }
  if (!raw) return empty;
  try {
    const parsed = JSON.parse(raw) as Partial<NewTopicDraft> | null;
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) return empty;
    return {
      title: str(parsed.title),
      body: str(parsed.body),
      repo: str(parsed.repo, empty.repo),
      base: str(parsed.base),
      profile: str(parsed.profile, empty.profile),
      agent: str(parsed.agent),
      model: str(parsed.model),
      effort: str(parsed.effort),
      mentions: Array.isArray(parsed.mentions)
        ? parsed.mentions
            .filter((m): m is NewTopicDraft["mentions"][number] =>
              !!m && typeof m === "object"
              && typeof (m as { token?: unknown }).token === "string"
              && typeof (m as { topicId?: unknown }).topicId === "string"
              && typeof (m as { resourceId?: unknown }).resourceId === "string")
        : [],
      attachments: Array.isArray(parsed.attachments)
        ? parsed.attachments
            .filter((a): a is FileAttachment =>
              !!a && typeof a === "object"
              && typeof a.name === "string" && a.name !== ""
              && typeof a.contentBase64 === "string"
              && typeof a.mimeType === "string")
            .map((a) => ({ name: a.name, size: a.size ?? 0, contentBase64: a.contentBase64, mimeType: a.mimeType }))
        : [],
    };
  } catch {
    return empty;
  }
}

/**
 * Snapshot the draft. Transient menu state (open comboboxes, mention menu,
 * highlight index) is component-local and deliberately not part of the
 * draft — reopen restores the text and chips, not cursor and focus.
 */
export function saveDraft(storage: DraftStorage, draft: NewTopicDraft): void {
  try {
    storage.setItem(NEW_TOPIC_DRAFT_KEY, JSON.stringify(draft));
  } catch {
    // Quota exceeded or storage disabled — the draft lives for this
    // session only; better a silent skip than a crash on close.
  }
}

/** Drop the draft (launch succeeded or the user explicitly cleared it). */
export function clearDraft(storage: DraftStorage): void {
  try {
    storage.removeItem(NEW_TOPIC_DRAFT_KEY);
  } catch {
    /* storage unavailable */
  }
}

/**
 * A topic launch closes the sheet — if that launch succeeded, the draft
 * was turned into a real topic and must not come back. A close for any
 * other reason (navigation, Esc, cancel) keeps the draft for reopening.
 * Launch failures never reach here (the sheet stays open with the
 * draft); this only runs when the sheet is actually unmounting on
 * success, so no "has content" check is needed.
 */
export function clearDraftOnLaunch(storage: DraftStorage): void {
  clearDraft(storage);
}

/**
 * Whether a draft carries anything worth restoring — text, mentions, or
 * attachments. A blank draft reopening blank is indistinguishable from a
 * fresh one, but this keeps explicit (e.g. future "clear draft" actions)
 * honest.
 */
export function draftHasContent(draft: NewTopicDraft): boolean {
  return !!(draft.title.trim() || draft.body.trim() || draft.mentions.length || draft.attachments.length);
}

/**
 * Reconcile a stored draft's launch config with the profiles and agents
 * the server reports right now. The selection model keeps every select
 * non-empty and valid, so a vanished profile falls back to the last
 * profile that still exists (falling all the way back to the default
 * route) and a vanished agent resets to the runtime default — launching
 * with a stale id would only fail at launch, after the user has lost the
 * sheet. Model and effort stay as typed: model is a free-form field with
 * a datalist, and effort simply re-selects. When the options are unknown
 * (still loading or loom unreachable), nothing is reconciled — resetting
 * a saved profile over missing metadata would silently relaunch under
 * the default route.
 */
export function mergeLaunchConfig(
  draft: NewTopicDraft,
  launchOptions: { profiles: { name: string; class?: string }[]; agents?: { kind: string }[] } | null | undefined,
): Pick<NewTopicDraft, "profile" | "agent" | "model" | "effort"> {
  const empty = emptyDraft();
  if (!launchOptions) {
    return { profile: draft.profile || empty.profile, agent: draft.agent, model: draft.model, effort: draft.effort };
  }
  const profiles = launchOptions.profiles?.filter((p) => p.class === "interactive") ?? [];
  return {
    profile: profiles.some((p) => p.name === draft.profile)
      ? draft.profile
      : profiles.length ? profiles[profiles.length - 1].name : empty.profile,
    agent: !draft.agent || launchOptions.agents == null || launchOptions.agents.some((a) => a.kind === draft.agent)
      ? draft.agent
      : empty.agent,
    model: draft.model || empty.model,
    effort: draft.effort || empty.effort,
  };
}
