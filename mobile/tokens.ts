/**
 * The phone's colours: system greys, and one tint for what can be tapped
 * (PHONE_COLOURS), in both themes. The status colours (success, warning,
 * error, info, violet) are the desktop's: the `--c-*` values of those names,
 * read from the desktop's stylesheet (app/src/index.css, their single source
 * of truth) when the app is built, never copied.
 *
 * The app writes the theme it shows to `<html data-theme>` (lib/theme.ts:
 * the phone's setting, or the user's choice). Before it does, the phone's
 * setting decides, dark when it says nothing.
 */
import { themeTokens } from '../app/src/lib/contrast.ts';

/** The desktop tokens the phone uses: what a state means. */
export const STATUS_TOKENS = ['success', 'success-soft', 'warning', 'warning-soft', 'error', 'info', 'violet'] as const;

export interface PhoneColours {
  /** The grouped background behind everything. */
  background: string;
  /** A cell of a grouped list, a sheet, the menu. */
  cell: string;
  /** Text. */
  label: string;
  /** Secondary text: subtitles, section headers, footnotes. */
  'label-2': string;
  /** Never text: chevrons, hairline glyphs. */
  'label-3': string;
  /** A field, a quiet button, a segmented control's track. */
  fill: string;
  /** Hairlines between rows and under bars. */
  separator: string;
  /** What can be tapped: links, the active tab, the send circle. */
  tint: string;
  /** A glyph or a word on the tint. */
  'on-tint': string;
  /** The inside of a folder icon, under its tinted outline. */
  'tint-fill': string;
  /** The translucent bars: the tab bar and the follow-up field. */
  glass: string;
}

/**
 * Light and dark: Apple's system greys, with two changes so that text reads
 * at 4.5:1 or more on every surface it sits on (tests/tokens.test.ts). Light
 * secondary text is rgba(60,60,67,0.76), where the system value of 0.6 is
 * 3.4:1 on white; and a glyph on the dark tint is black, as white on #ff8a4c
 * is 2.3:1.
 */
export const PHONE_COLOURS: Record<'light' | 'dark', PhoneColours> = {
  light: {
    background: '#f2f2f7',
    cell: '#ffffff',
    label: '#000000',
    'label-2': 'rgba(60, 60, 67, 0.76)',
    'label-3': 'rgba(60, 60, 67, 0.3)',
    fill: 'rgba(118, 118, 128, 0.12)',
    separator: 'rgba(60, 60, 67, 0.2)',
    tint: '#c2410c',
    'on-tint': '#ffffff',
    'tint-fill': 'rgba(194, 65, 12, 0.08)',
    glass: 'rgba(249, 249, 249, 0.84)',
  },
  dark: {
    background: '#000000',
    cell: '#1c1c1e',
    label: '#ffffff',
    'label-2': 'rgba(235, 235, 245, 0.6)',
    'label-3': 'rgba(235, 235, 245, 0.3)',
    fill: 'rgba(118, 118, 128, 0.24)',
    separator: 'rgba(84, 84, 88, 0.6)',
    tint: '#ff8a4c',
    'on-tint': '#000000',
    'tint-fill': 'rgba(255, 138, 76, 0.12)',
    glass: 'rgba(22, 22, 24, 0.84)',
  },
};

/** The status tokens of one theme of the desktop's index.css; every one must be there. */
export function statusTokens(desktopCss: string, selector: string): Record<string, string> {
  const all = themeTokens(desktopCss, selector);
  for (const name of STATUS_TOKENS) if (!all[name]) throw new Error(`The desktop has no --c-${name} for ${selector}`);
  return Object.fromEntries(STATUS_TOKENS.map(name => [name, all[name]]));
}

function declarations(phone: PhoneColours, status: Record<string, string>, indent: string): string {
  return [
    ...Object.entries(phone).map(([name, value]) => `${indent}--${name}: ${value};`),
    ...Object.entries(status).map(([name, value]) => `${indent}--c-${name}: ${value};`),
  ].join('\n');
}

/** The phone's token stylesheet, its status colours read from the desktop's index.css text. */
export function tokensCss(desktopCss: string): string {
  const dark = statusTokens(desktopCss, '[data-theme="dark"]');
  const light = statusTokens(desktopCss, '[data-theme="light"]');
  return [
    ':root {',
    '  color-scheme: dark;',
    declarations(PHONE_COLOURS.dark, dark, '  '),
    '}',
    '@media (prefers-color-scheme: light) {',
    '  :root:not([data-theme]) {',
    '    color-scheme: light;',
    declarations(PHONE_COLOURS.light, light, '    '),
    '  }',
    '}',
    ':root[data-theme="light"] {',
    '  color-scheme: light;',
    declarations(PHONE_COLOURS.light, light, '  '),
    '}',
    '',
  ].join('\n');
}
