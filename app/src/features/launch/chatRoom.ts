/**
 * Whether the sidebar has room for the chats: the launch test's measure, in each view of
 * its tour (`launch_probe.rs`), of how many chat rows are whole on screen. A row is whole
 * when no box that hides or scrolls its content (the chat list, the sidebar's scroller),
 * and not the window, cuts any of it.
 *
 * The rule (`wholeRows`) takes numbers only, so the Node tests hold it; `chatRoom` reads
 * those numbers from a page.
 */

/** A box's top and bottom on screen. */
export interface Span {
  top: number;
  bottom: number;
}

/** How many of `rows` are wholly inside `clip`. */
export function wholeRows(rows: readonly Span[], clip: Span): number {
  return rows.filter(row => row.top >= clip.top && row.bottom <= clip.bottom).length;
}

/** The part of the window that `el`'s scrolling and hiding ancestors leave it. */
function clipOf(el: HTMLElement, view: Window): Span {
  let clip: Span = { top: 0, bottom: view.innerHeight };
  for (let box = el.parentElement; box; box = box.parentElement) {
    const overflowY = view.getComputedStyle(box).overflowY;
    if (overflowY === 'visible') continue;
    const rect = box.getBoundingClientRect();
    clip = { top: Math.max(clip.top, rect.top), bottom: Math.min(clip.bottom, rect.bottom) };
  }
  return clip;
}

/** The sidebar's chat rows that are drawn (`data-chat-row`), and how many are whole on screen. */
export function chatRoom(doc: Document): { listed: number; whole: number } {
  const view = doc.defaultView;
  if (!view) return { listed: 0, whole: 0 };
  const rows = Array.from(doc.querySelectorAll<HTMLElement>('[data-chat-row]')).filter(row => {
    const style = view.getComputedStyle(row);
    const rect = row.getBoundingClientRect();
    return style.visibility === 'visible' && style.display !== 'none' && rect.height > 1;
  });
  if (rows.length === 0) return { listed: 0, whole: 0 };
  const clip = clipOf(rows[0], view);
  const spans = rows.map(row => {
    const rect = row.getBoundingClientRect();
    return { top: rect.top, bottom: rect.bottom };
  });
  return { listed: rows.length, whole: wholeRows(spans, clip) };
}
