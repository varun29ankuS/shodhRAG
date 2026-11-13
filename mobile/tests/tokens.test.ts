import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import { themeTokens } from '../../app/src/lib/contrast.ts';
import { tokensCss } from '../tokens';

const desktop = readFileSync(new URL('../../app/src/index.css', import.meta.url), 'utf8');

describe("the phone's colours", () => {
  it('are the desktop tokens of both themes, dark by default', () => {
    const css = tokensCss(desktop);
    const [dark, light] = css.split('@media (prefers-color-scheme: light)');
    for (const [name, value] of Object.entries(themeTokens(desktop, '[data-theme="dark"]'))) {
      expect(dark).toContain(`--c-${name}: ${value};`);
    }
    for (const [name, value] of Object.entries(themeTokens(desktop, '[data-theme="light"]'))) {
      expect(light).toContain(`--c-${name}: ${value};`);
    }
    expect(dark).toContain('color-scheme: dark;');
    expect(light).toContain('color-scheme: light;');
  });
});
