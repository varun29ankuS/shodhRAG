/**
 * CI's phone tours (.github/workflows/ci.yml): iOS takes the same screens as Android, and
 * every demo screen they ask for opens where it should, in the state it shows: the link
 * failed, the PIN lock to set, a file shared to Shodh, Calendar settings, the end of a
 * long answer. Demo screens run only in debug builds (src/lib/demo.ts).
 */
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';
import { demoDevice, demoNav, demoScroll, demoStatus } from '../src/lib/demo';
import type { Device } from '../src/lib/device';

const ci = readFileSync(resolve(process.cwd(), '../.github/workflows/ci.yml'), 'utf8');

function androidShots(): string[] {
  const block = /shots=\(\n([\s\S]*?)\n\s*\)/.exec(ci);
  expect(block).not.toBeNull();
  return [...(block?.[1] ?? '').matchAll(/"([^"]+)"/g)].map(m => m[1]);
}

function iosShots(): string[] {
  const line = /for shot in (pair:pair:pair [^;]*); do/.exec(ci);
  expect(line).not.toBeNull();
  return (line?.[1] ?? '').trim().split(/\s+/);
}

describe('tour', () => {
  it('takes the same screens on iOS as on Android', () => {
    const android = androidShots();
    // Android starts at the app's own start; iOS launches the pair screen by name.
    expect(iosShots()).toEqual(android.map(s => (s === 'pair:start:pair' ? 'pair:pair:pair' : s)));
    expect(android.length).toBeGreaterThanOrEqual(26);
  });

  it('opens every demo screen it asks for', () => {
    for (const shot of androidShots()) {
      const [, request] = shot.split(':');
      if (!request.startsWith('demo-')) continue;
      expect(demoNav(request.slice('demo-'.length)), request).not.toBeNull();
    }
  });

  it('shows each state its screen is for', async () => {
    expect(demoStatus('failed').state).toBe('failed');
    expect(demoStatus('home').state).toBe('connected');
    expect(demoScroll('answer-end')).toBe('end');
    expect(demoScroll('settings-calendar')).toBe('settings-calendar');
    expect(demoScroll('home')).toBeNull();

    const base = { lock: { status: async () => ({ required: false }) }, files: { takeIncoming: async () => ({ kind: 'none' }) } } as unknown as Device;
    const lock = await demoDevice('lock', base).lock.status();
    expect(lock).toMatchObject({ required: true, hasPin: false, locked: true });
    const sending = demoDevice('send', base).files;
    expect(await sending.takeIncoming()).toMatchObject({ kind: 'files' });
    // Shared once: taken again, nothing is waiting.
    expect(await sending.takeIncoming()).toEqual({ kind: 'none' });
    expect(demoDevice('home', base)).toBe(base);
  });

  it('keeps the follow-up bar\'s measured height clear under the answer', () => {
    const css = readFileSync(resolve(process.cwd(), 'src/theme.css'), 'utf8');
    expect(css).toMatch(/\.scroll-with-composer \{\s*padding-bottom: calc\(50px \+ 8px \+ var\(--askbox-h, 48px\) \+ 32px \+ var\(--inset-bottom\)\);/);
    const ask = readFileSync(resolve(process.cwd(), 'src/screens/Ask.tsx'), 'utf8');
    expect(ask).toMatch(/<form ref=\{askboxRef\} className="askbox askbox-floating"/);
    expect(ask).toMatch(/\['--askbox-h' as string\]: `\$\{askboxHeight\}px`/);
  });
});
