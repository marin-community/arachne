// Selection → textContent offset mapping for markdown copy.
//
// The copy handler needs to know where a selection's edges fall inside a
// rendered message body, in textContent-relative characters — the same
// space markdownCopy's mapping consumes. This module holds that DOM glue
// so it can be tested outside the component.

/** Where the selection's edge falls inside a rendered body, in
 * textContent-relative characters (0 = body start, length = body end). */
export function bodyOffset(range: Range, body: HTMLElement, edge: "start" | "end"): number {
  // A probe range spanning from the body's start to the selection edge
  // (or from the selection end to the body's end, then converted) measures
  // the offset in characters of rendered text. Edges outside the body
  // clamp to 0 / full length (setEnd before the probe's start would
  // collapse it the wrong way).
  const probe = document.createRange();
  probe.selectNodeContents(body);
  if (edge === "start") {
    if (range.compareBoundaryPoints(Range.START_TO_START, probe) <= 0) return 0;
    probe.setEnd(range.startContainer, range.startOffset);
    return probe.toString().length;
  }
  if (range.compareBoundaryPoints(Range.END_TO_END, probe) >= 0) return (body.textContent ?? "").length;
  probe.setStart(range.endContainer, range.endOffset);
  // The probe spans from the selection end to the body's end; convert
  // the measured suffix length into the selection end's offset.
  return (body.textContent ?? "").length - probe.toString().length;
}
