/**
 * Which phone this is. The app is one for Android and iPhone; the few places
 * where the two differ read it here: the name of the native plugin whose
 * events the window listens to, how the pairing code is scanned, and the
 * words that name the system ("Android asks you to confirm it").
 */
export type PhoneOS = 'android' | 'ios';

/** The system, from what its WebView says of itself. */
export function detectOS(userAgent: string, platform: string, touchPoints: number): PhoneOS {
  if (/iPhone|iPad|iPod/.test(userAgent)) return 'ios';
  // An iPad asks for desktop sites by saying it is a Mac; a Mac has no touch screen.
  if (platform === 'MacIntel' && touchPoints > 1) return 'ios';
  return 'android';
}

export const OS: PhoneOS = detectOS(navigator.userAgent, navigator.platform, navigator.maxTouchPoints ?? 0);

export const IOS = OS === 'ios';

/** The system's name, in a sentence. */
export const SYSTEM = IOS ? 'iOS' : 'Android';

/** Shodh's native plugin on this phone (mobile/plugins): its events are the same on both. */
export const PLUGIN = IOS ? 'shodh-ios' : 'shodh-android';

/** The lock's words for how this phone reads its user (`LockStatus.biometry`). */
export const BIOMETRY = {
  fingerprint: { unlock: 'Use your fingerprint', on: 'Use fingerprint', off: 'No fingerprint' },
  face: { unlock: 'Use Face ID', on: 'Use Face ID', off: 'No Face ID' },
  touch: { unlock: 'Use Touch ID', on: 'Use Touch ID', off: 'No Touch ID' },
} as const;
