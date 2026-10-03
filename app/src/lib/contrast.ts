/**
 * WCAG 2.x contrast helpers and a parser for the theme tokens in
 * src/index.css. Pure module (no runtime imports) so the token contrast
 * audit runs directly with Node (`app/tests/contrast.test.ts`).
 */

/** Parse "#rrggbb" into 0–255 channels. */
export function parseHex(hex: string): [number, number, number] {
  const m = /^#([0-9a-f]{6})$/i.exec(hex.trim());
  if (!m) throw new Error(`Not a 6-digit hex colour: ${hex}`);
  const n = parseInt(m[1], 16);
  return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
}

/** WCAG relative luminance. */
export function luminance(hex: string): number {
  const [r, g, b] = parseHex(hex).map(c => {
    const s = c / 255;
    return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
  });
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

/** WCAG contrast ratio between two colours (1–21). */
export function contrastRatio(a: string, b: string): number {
  const la = luminance(a);
  const lb = luminance(b);
  const [hi, lo] = la > lb ? [la, lb] : [lb, la];
  return (hi + 0.05) / (lo + 0.05);
}

/**
 * The `--c-*` tokens of one theme block in index.css. `selector` is matched
 * against the block's selector list, e.g. '[data-theme="light"]'.
 */
export function themeTokens(css: string, selector: string): Record<string, string> {
  const blocks = css.matchAll(/([^{}]+)\{([^{}]*)\}/g);
  for (const [, selectors, body] of blocks) {
    const list = selectors.split(',').map(s => s.trim());
    if (!list.includes(selector)) continue;
    const tokens: Record<string, string> = {};
    for (const [, name, value] of body.matchAll(/--c-([a-z0-9-]+)\s*:\s*(#[0-9a-fA-F]{6})\s*;/g)) {
      tokens[name] = value;
    }
    if (Object.keys(tokens).length > 0) return tokens;
  }
  throw new Error(`No token block for ${selector}`);
}
