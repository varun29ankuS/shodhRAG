/**
 * An HTML block in an answer, previewed inline: the page as the model wrote it, in a frame
 * that runs no scripts and loads nothing from the network, the same lock as the provider
 * attribution frame (`AttributionFrame`: `sandbox=""`, no same origin, no forms, no popups,
 * no top navigation). The frame's own policy adds that only inline styles and `data:`
 * images draw, so a page never reaches the network or the app.
 *
 * Pure, so it is unit-tested directly with Node (`app/tests/htmlPreview.test.ts`).
 */

/** The sandbox the inline preview's frame gets: every permission off. */
export const HTML_PREVIEW_SANDBOX = '';

/** The policy written into the previewed page itself. */
export const HTML_PREVIEW_POLICY =
  "default-src 'none'; style-src 'unsafe-inline'; img-src data:; font-src data:; form-action 'none'; base-uri 'none'";

/**
 * The preview's height. The frame runs no scripts, so the page cannot say how tall it is:
 * the frame takes 60% of the window's height, never under 360 px or over 720 px, and the
 * user can drag it taller or shorter. Its width is the wide block's (`wideBlock.ts`).
 */
export const HTML_PREVIEW_HEIGHT_CLASS = 'html-preview-frame';

/** Languages of a fenced block shown as a page preview. */
export function isHtmlLanguage(language: string): boolean {
  const lang = language.toLowerCase();
  return lang === 'html' || lang === 'htm' || lang === 'xhtml';
}

const POLICY_META = `<meta http-equiv="Content-Security-Policy" content="${HTML_PREVIEW_POLICY}">`;

/**
 * The frame's document: the policy goes first in its head, before anything the page
 * declares, so a page cannot loosen it (the strictest of several policies applies).
 */
export function previewDocument(source: string): string {
  const head = /<head(\s[^>]*)?>/i.exec(source);
  if (head) {
    const at = head.index + head[0].length;
    return `${source.slice(0, at)}${POLICY_META}${source.slice(at)}`;
  }
  const html = /<html(\s[^>]*)?>/i.exec(source);
  if (html) {
    const at = html.index + html[0].length;
    return `${source.slice(0, at)}<head>${POLICY_META}</head>${source.slice(at)}`;
  }
  return `<!doctype html><html><head><meta charset="utf-8">${POLICY_META}</head><body>${source}</body></html>`;
}
