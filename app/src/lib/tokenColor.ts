/**
 * A colour token's value as the document resolves it now: `--c-<name>` in `index.css`,
 * for the theme on screen. For the few places that cannot take a class or a CSS variable
 * (a chart library's SVG attributes). Everything else uses the token's class
 * (`text-shodh-text`) or `var(--c-<name>)`.
 */
export function tokenColor(name: string): string {
  return getComputedStyle(document.documentElement).getPropertyValue(`--c-${name}`).trim();
}
