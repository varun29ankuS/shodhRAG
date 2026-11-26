/**
 * The one app on two phones: which phone it is (lib/platform.ts), and what an
 * iPhone shows differently: no Push (iOS has no UnifiedPush) and Briefly in
 * place of Stay connected (iOS suspends an app soon after it leaves the
 * screen), the system named as iOS, and the lock named Face ID or Touch ID.
 */
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { memoryDevice } from './device';

vi.mock('../src/lib/platform', async importOriginal => {
  const actual = await importOriginal<typeof import('../src/lib/platform')>();
  return { ...actual, OS: 'ios', IOS: true, SYSTEM: 'iOS', PLUGIN: 'shodh-ios' };
});

const { detectOS, BIOMETRY } = await import('../src/lib/platform');
const { NoteProvider } = await import('../src/lib/context');
const { DeviceProvider } = await import('../src/lib/queue');
const { LockSettings, NotificationSettings } = await import('../src/components/PrivacySettings');
const { LockScreen } = await import('../src/components/LockScreen');
const { UpdatePanel } = await import('../src/components/Update');

describe('which phone this is', () => {
  it('is an iPhone or an iPad by its WebView, and Android otherwise', () => {
    expect(detectOS('Mozilla/5.0 (iPhone; CPU iPhone OS 17_5 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Mobile/15E148', 'iPhone', 5)).toBe('ios');
    expect(detectOS('Mozilla/5.0 (iPad; CPU OS 17_5 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Mobile/15E148', 'iPad', 5)).toBe('ios');
    // An iPad asking for desktop sites says it is a Mac, with a touch screen.
    expect(detectOS('Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko)', 'MacIntel', 5)).toBe('ios');
    expect(detectOS('Mozilla/5.0 (Linux; Android 14; Pixel 8 Build/UQ1A; wv) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0 Mobile Safari/537.36', 'Linux armv81', 5)).toBe('android');
    expect(detectOS('Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko)', 'MacIntel', 0)).toBe('android');
  });
});

describe('on an iPhone', () => {
  it('Notifications are Off or Briefly, never Push, and Briefly says how long', async () => {
    const phone = memoryDevice();
    render(
      <DeviceProvider device={phone}>
        <NoteProvider>
          <NotificationSettings />
        </NoteProvider>
      </DeviceProvider>,
    );
    const group = await screen.findByRole('radiogroup', { name: 'Notifications' });
    await waitFor(() => expect(within(group).getByRole('radio', { name: 'Off' }).getAttribute('aria-checked')).toBe('true'));
    expect(within(group).getAllByRole('radio').map(radio => radio.textContent?.trim())).toEqual(['Off', 'Briefly']);
    fireEvent.click(within(group).getByRole('radio', { name: 'Briefly' }));
    await waitFor(() => expect(phone.notifications.sets).toEqual([['stay', undefined]]));
    expect(await screen.findByText(/about 30 seconds after you leave it/)).toBeTruthy();
  });

  it('the update is looked for as the iOS app', async () => {
    const phone = memoryDevice({ update: { state: 'unavailable', message: 'Update Shodh to 0.0.12 in TestFlight or the App Store, then open it again.' } });
    render(
      <DeviceProvider device={phone}>
        <UpdatePanel desktop="0.0.12" />
      </DeviceProvider>,
    );
    expect(await screen.findByText('Update Shodh to 0.0.12 in TestFlight or the App Store, then open it again.')).toBeTruthy();
    // Nothing to download on an iPhone: the one action is to look again.
    expect(screen.getByRole('button', { name: 'Look again' })).toBeTruthy();
    expect(screen.queryByRole('button', { name: /Update to/ })).toBeNull();
  });

  it('the lock names Face ID or Touch ID, whichever the phone has', async () => {
    for (const biometry of ['face', 'touch'] as const) {
      const phone = memoryDevice();
      phone.lock.status_ = { required: true, hasPin: true, biometric: true, biometry, locked: true };
      const lock = render(
        <DeviceProvider device={phone}>
          <LockScreen status={phone.lock.status_} onOpen={() => undefined} />
        </DeviceProvider>,
      );
      expect(screen.getByRole('button', { name: BIOMETRY[biometry].unlock })).toBeTruthy();
      lock.unmount();
      phone.lock.status_ = { ...phone.lock.status_, locked: false };
      const settings = render(
        <DeviceProvider device={phone}>
          <NoteProvider>
            <LockSettings />
          </NoteProvider>
        </DeviceProvider>,
      );
      expect(await screen.findByRole('button', { name: BIOMETRY[biometry].off })).toBeTruthy();
      settings.unmount();
    }
    expect(BIOMETRY.face.unlock).toBe('Use Face ID');
    expect(BIOMETRY.touch.unlock).toBe('Use Touch ID');
  });
});
