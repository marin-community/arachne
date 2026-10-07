// OS notifications for Needs You attention (docs/design.md: the cockpit
// must reach the person, not only the window they might have left).
//
// Tiering, in order: Tauri's notification plugin (macOS Notification
// Center) when running inside the desktop shell, the browser
// Notification API as a secondary path, and finally no OS notification
// at all (the in-app AttentionBanner always remains). The pure
// identity/dedup logic lives here so it is unit-testable without a
// Tauri runtime.

import type { AttentionNotice } from "./attentionNotifications";

/** A tiny injectable notifier so tests can intercept sends. */
export type OsNotifier = (title: string, body: string) => void;

/** How OS notifications are (or could be) delivered right now. */
export type OsNotificationState = {
  /** OS notifications will actually be shown. */
  available: boolean;
  /** Why not, when `available` is false. */
  reason?: "no-runtime" | "denied";
  /** Send path in use; matches the debug hint in the settings sheet. */
  transport?: "tauri" | "web";
};

export const OS_NOTIFICATIONS_SETTING = "arachne.osNotifications";

export function osNotificationsEnabled(storage: Storage): boolean {
  return storage.getItem(OS_NOTIFICATIONS_SETTING) !== "false";
}

/** Probe the transport once per runtime. Never throws. */
export async function probeOsNotifications(): Promise<OsNotificationState> {
  try {
    const { isTauri } = await import("@tauri-apps/api/core");
    if (isTauri()) {
      const plugin = await import("@tauri-apps/plugin-notification");
      let granted = await plugin.isPermissionGranted();
      if (!granted) {
        granted = (await plugin.requestPermission()) === "granted";
      }
      return granted
        ? { available: true, transport: "tauri" }
        : { available: false, reason: "denied" };
    }
  } catch {
    // Not inside Tauri, or the plugin is unavailable: fall through.
  }
  if (typeof window !== "undefined" && "Notification" in window) {
    // The web tier only serves plain-browser development, where an
    // unprompted "granted" is rare; never nag for permission — the Tauri
    // tier is the real path.
    return window.Notification.permission === "denied"
      ? { available: false, reason: "denied" }
      : { available: true, transport: "web" };
  }
  return { available: false, reason: "no-runtime" };
}

/** Build the notifier for the probed state. Never throws. */
export function osNotifier(state: OsNotificationState): OsNotifier {
  if (!state.available) return () => {};
  return (title, body) => {
    try {
      if (state.transport === "tauri") {
        void import("@tauri-apps/plugin-notification").then((plugin) =>
          plugin.sendNotification({ title, body }),
        );
      } else if (typeof window !== "undefined" && "Notification" in window
        && window.Notification.permission === "granted") {
        // Only an already-granted web permission sends — never prompt here.
        new window.Notification(title, { body });
      }
    } catch {
      // A failed send must never break the app; the banner still shows.
    }
  };
}

/** The first send of each notice. A notice already shown is skipped, a
 *  recurring one (key changed — new request, note, or tag) sends again.
 *  Keyed like the banner: identity includes the request, not activity.
 *  Resolved keys are pruned (like `retainActiveDismissals`) so the set stays
 *  bounded and a later recurrence with the same key announces again. */
export function announceNotices(
  notices: readonly AttentionNotice[],
  shown: ReadonlySet<string>,
  send: OsNotifier,
): Set<string> {
  const next = new Set(notices.map((n) => n.key));
  for (const notice of notices) {
    if (shown.has(notice.key)) continue;
    send(notice.level === "blocked" ? "Needs you — blocked" : "Needs you", `${notice.title}: ${notice.reason}`);
  }
  return next;
}
