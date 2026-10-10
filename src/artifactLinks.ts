export interface ArtifactTarget {
  /** Canonical branch id from artifacts.url, or a legacy session key. */
  key: string;
  name: string;
}

/** Only Loom's artifact page route on the configured HTTP(S) origin.
 * API endpoint paths in settings do not change Loom's root-relative UI routes. */
export function loomArtifactTarget(href: string, loomUrl: string): ArtifactTarget | null {
  try {
    const base = new URL(loomUrl);
    if (!['http:', 'https:'].includes(base.protocol)) return null;
    const value = href.trim();
    // Relative links must be explicit root-relative Loom routes, never WebView URLs.
    if (!/^(?:https?:\/\/|\/)/i.test(value) || value.includes('\\')) return null;
    const url = new URL(value, base.origin);
    if (url.origin !== base.origin || url.username || url.password) return null;
    const match = /^\/s\/([^/]+)\/artifacts\/([^/]+)\/?$/.exec(url.pathname);
    if (!match) return null;
    const key = decodeURIComponent(match[1]);
    const name = decodeURIComponent(match[2]);
    if (!key || /[\x00-\x1f\x7f/\\]/.test(key) || !name || /[\x00-\x1f\x7f]/.test(name)) return null;
    return { key, name };
  } catch {
    return null;
  }
}

/** Prevent browser navigation only after recognizing a configured artifact. */
export function routeArtifactClick(
  event: { defaultPrevented: boolean; preventDefault(): void },
  href: string,
  loomUrl: string,
  open: (target: ArtifactTarget) => void,
): boolean {
  if (event.defaultPrevented) return false;
  const target = loomArtifactTarget(href, loomUrl);
  if (!target) return false;
  event.preventDefault();
  open(target);
  return true;
}
