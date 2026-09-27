// Markdown-preserving paste for the composers.
//
// The conversation's copy handler writes two flavors: text/plain = the raw
// markdown of the selected turns, text/html = the rendered markup (rich
// text targets keep formatting). But WebKit's default paste into a
// <textarea> prefers the text/html flavor and flattens it — a copied turn
// pastes as rendered text ("• bullets", no fences, links as inline text)
// and the markdown is lost.
//
// The composers are plain-text editors whose whole content is markdown, so
// the paste side must take the text/plain flavor when one exists. Only
// when there is no text/plain (an image paste, or text copied from an app
// that only writes HTML) does the default behavior run.

/** True when the clipboard carries a text/plain flavor worth pasting. */
export function pasteAsPlainText(event: ClipboardEvent): boolean {
  const data = event.clipboardData;
  if (!data) return false;
  if (filesPresent(data)) return false;
  if (!data.types.includes("text/plain")) return false;
  // Some sources (Safari copying a PDF selection) advertise text/plain but
  // leave it empty; pasting that would drop the content entirely.
  const text = data.getData("text/plain");
  return text.length > 0;
}

/** Files in the paste (a screenshot, or a copied file from Finder). */
function filesPresent(data: DataTransfer): boolean {
  if (data.files?.length) return true;
  return Array.from(data.items ?? []).some((item) => item.kind === "file");
}
