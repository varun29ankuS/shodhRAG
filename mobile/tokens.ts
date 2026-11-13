/**
 * The phone uses the desktop's colour tokens: the `--c-*` values of both
 * themes, read from the desktop's stylesheet (app/src/index.css, the single
 * source of truth) when the app is built, never copied. Dark is the default,
 * and the phone's light setting switches to the light values.
 */
import { themeTokens } from '../app/src/lib/contrast.ts';

const declarations = (tokens: Record<string, string>, indent: string) =>
  Object.entries(tokens)
    .map(([name, value]) => `${indent}--c-${name}: ${value};`)
    .join('\n');

/** The phone's token stylesheet from the desktop's index.css text. */
export function tokensCss(desktopCss: string): string {
  const dark = themeTokens(desktopCss, '[data-theme="dark"]');
  const light = themeTokens(desktopCss, '[data-theme="light"]');
  return [
    ':root {',
    '  color-scheme: dark;',
    declarations(dark, '  '),
    '}',
    '@media (prefers-color-scheme: light) {',
    '  :root {',
    '    color-scheme: light;',
    declarations(light, '    '),
    '  }',
    '}',
    '',
  ].join('\n');
}
