export interface ActivityBinding { id: string; kind: string; label: string; target_session_id?: string | null }
export interface ActivityChannel {
  id: string; name: string; kind: string; session_id?: string | null; state: string;
  bindings?: ActivityBinding[];
}
export interface ActivityDelivery {
  binding_id: string; binding_kind: string; target_session_id?: string | null;
  state: string; attempts: number; last_error?: string | null;
}
export interface ActivityMessage {
  id: string; channel_id: string; seq: number; kind: string; urgency: string;
  author_kind: string; author_id: string; body: string; created_at: string;
  deliveries?: ActivityDelivery[];
}
export interface ActivityWatch {
  id: string; name: string; enabled: boolean;
  trigger: { on?: string[]; event?: string; level?: string; cron?: string; every?: string; repo?: string };
  scope: { repo?: string; attention?: string };
  last_outcome?: string | null; last_run_at?: string | null; next_run_at?: string | null;
  wake_at?: string | null;
  latest_run?: { summary?: string; stderr?: string; exit_code?: number | null } | null;
}
export interface TrackActivitySnapshot {
  channels: ActivityChannel[]; messages: ActivityMessage[]; watches: ActivityWatch[];
  warnings: string[]; channels_truncated: boolean;
}

export function watchTriggerLabel(watch: ActivityWatch): string {
  const trigger = watch.trigger ?? {};
  const parts: string[] = [];
  if (trigger.every) parts.push(`Every ${trigger.every}`);
  if (trigger.cron) parts.push(`Schedule: ${trigger.cron}`);
  const events = [...(trigger.on ?? [])];
  if (trigger.event) events.push(`${trigger.event}${trigger.level ? `=${trigger.level}` : ""}`);
  if (events.length) parts.push(`On ${[...new Set(events)].join(", ")}`);
  return parts.join(" · ") || "Manual trigger";
}

export function watchTriggerKind(watch: ActivityWatch): string {
  const t = watch.trigger ?? {};
  const timer = Boolean(t.cron || t.every);
  const event = Boolean(t.event || t.on?.length);
  return timer && event ? "Events + timer" : timer ? "Timer" : event ? "Events" : "Manual";
}

/** A stored message is not proof that an agent ran or successfully handled it. */
export function deliverySummary(message: ActivityMessage): string {
  const deliveries = message.deliveries ?? [];
  if (deliveries.some((d) => d.last_error || d.state === "failed" || d.state === "error")) return "Delivery failed";
  if (!deliveries.length) return message.kind === "result" ? "Result recorded" : "Recorded";
  if (deliveries.every((d) => d.state === "delivered")) return "Delivered";
  return [...new Set(deliveries.map((d) => d.state))].join(" · ");
}
