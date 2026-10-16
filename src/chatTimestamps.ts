import type { ChatRow } from "./chatRows.ts";

const GAP_MS = 5 * 60 * 1000;

function timestamp(value?: string | null): number | null {
  const time = value ? Date.parse(value) : NaN;
  return Number.isFinite(time) ? time : null;
}

/** Add separators to visible rows without changing their stable journal keys.
 * A work group's entire time span counts as activity, even when collapsed. */
export function withChatTimestamps<T extends ChatRow>(
  rows: readonly T[],
): (T & { timestamp: string | null })[] {
  let previous: number | null = null;
  return rows.map((row) => {
    const blocks = row.kind === "work_group" ? row.blocks : [row.block];
    const times = blocks.map((block) => timestamp(block.created_at))
      .filter((time): time is number => time != null);
    if (!times.length) return { ...row, timestamp: null };

    const first = times[0];
    const show = previous == null || first - previous >= GAP_MS ||
      new Date(first).toDateString() !== new Date(previous).toDateString();
    // A late/out-of-order block must not move the activity clock backwards.
    previous = Math.max(previous ?? first, ...times);
    return { ...row, timestamp: show ? new Date(first).toISOString() : null };
  });
}

/** Local calendar days (rather than 24-hour intervals) handle DST correctly. */
export function formatChatTimestamp(value: string, now = new Date(), locale?: string): string {
  const time = new Date(value);
  if (!Number.isFinite(time.getTime())) return "";
  const yesterday = new Date(now);
  yesterday.setDate(yesterday.getDate() - 1);
  const day = time.toDateString() === now.toDateString() ? "Today"
    : time.toDateString() === yesterday.toDateString() ? "Yesterday"
    : time.toLocaleDateString(locale, {
      weekday: "short", month: "short", day: "numeric",
      ...(time.getFullYear() !== now.getFullYear() ? { year: "numeric" as const } : {}),
    });
  return `${day}, ${time.toLocaleTimeString(locale, { hour: "numeric", minute: "2-digit" })}`;
}
