/** The splash ("Connecting to your computer") shows the whole elephant from its first frame. */
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { Splash } from '../src/components/Splash';

const css = readFileSync(resolve(process.cwd(), 'src/theme.css'), 'utf8');

describe('the splash', () => {
  it('shows the elephant at once, whole, on the theme\u2019s own background', () => {
    render(<Splash />);
    const logo = screen.getByRole('img', { name: 'Shodh' });
    expect(logo.classList.contains('splash-logo')).toBe(true);
    expect(logo.getAttribute('src')).toBeTruthy();
    // Never hidden while it springs in: only its size settles.
    expect(logo.style.opacity === '' || Number(logo.style.opacity) === 1).toBe(true);
    expect(screen.getByText('Connecting to your computer')).toBeTruthy();
    const rule = /\.splash-logo\s*\{([^}]*)\}/.exec(css)?.[1] ?? '';
    expect(rule).toMatch(/object-fit:\s*contain/);
    expect(/\.splash\s*\{([^}]*)\}/.exec(css)?.[1]).toMatch(/background:\s*var\(--splash\)/);
    expect(css).toMatch(/--splash:\s*#ffffff/);
    expect(css).toMatch(/--splash:\s*#0b0b0d/);
  });
});
