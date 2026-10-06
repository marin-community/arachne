/** Keep Loom's durable topic marker for compatibility with existing tracks. */
export function isStandalone(session: { branch: { tags?: { key: string; value: string }[] } }): boolean {
  return session.branch.tags?.some(tag => tag.key === "topic" && tag.value === "false") ?? false;
}
export function isTrack(session: Parameters<typeof isStandalone>[0]): boolean {
  return !isStandalone(session);
}
