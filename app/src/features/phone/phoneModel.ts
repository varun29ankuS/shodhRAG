/**
 * Settings → Phone: the shapes of the `phone_*` commands (`phone_link.rs`) and the
 * words the page shows.
 *
 * Pure module (no runtime imports) so it is unit-tested directly with Node
 * (`app/tests/phoneModel.test.ts`).
 */

/** The event the desktop sends when a phone pairs or is removed. */
export const PHONE_DEVICES_CHANGED = 'phone_devices_changed';

/** A QR code's modules, row by row (`1` dark). */
export interface QrMatrix {
  size: number;
  rows: string[];
}

/** `phone_pairing_start`: the QR code to scan and when it stops working. */
export interface PairingCode {
  qr: QrMatrix;
  expiresAt: string;
}

/** One of `phone_devices`. */
export interface PairedPhone {
  nodeId: string;
  name: string;
  pairedAt: string;
  lastSeen: string | null;
}

/** An error of a `phone_*` command, in words. */
export function errorText(err: unknown): string {
  if (typeof err === 'string') return err;
  if (err instanceof Error) return err.message;
  if (typeof err === 'object' && err !== null && typeof (err as { message?: unknown }).message === 'string') {
    return (err as { message: string }).message;
  }
  return String(err);
}

function day(iso: string, locale?: string): string | null {
  const t = Date.parse(iso);
  if (Number.isNaN(t)) return null;
  return new Date(t).toLocaleDateString(locale, { day: 'numeric', month: 'short', year: 'numeric' });
}

/** "Paired 13 Nov 2026 · last connected 14 Nov 2026". */
export function whenSeen(phone: PairedPhone, locale?: string): string {
  const paired = day(phone.pairedAt, locale);
  const seen = phone.lastSeen ? day(phone.lastSeen, locale) : null;
  const parts = [paired ? `Paired ${paired}` : 'Paired'];
  if (seen) parts.push(`last connected ${seen}`);
  return parts.join(' · ');
}

/** The event the desktop sends when "Stay available for my phone" or what it does changed. */
export const PHONE_STAY_CHANGED = 'phone_stay_available_changed';

/** `phone_stay_available`: the setting and what it does now (`stay_available.rs`). */
export interface StayStatus {
  enabled: boolean;
  /** This computer can be kept awake by Shodh (Windows and macOS). */
  supported: boolean;
  /** Mains power: `null` when the computer cannot tell. */
  onAc: boolean | null;
  /** The computer is being kept awake now. */
  holding: boolean;
  localOnly: boolean;
  paired: boolean;
}

/** What "Stay available for my phone" does now, in one sentence. */
export function stayWords(status: StayStatus): string {
  if (!status.supported) return 'Shodh cannot keep this computer awake on this system.';
  if (!status.enabled) return 'Off. This computer sleeps as usual, and your phone waits until it wakes.';
  if (status.localOnly) return 'Paused: Local-only mode is on, so your phone cannot reach this computer.';
  if (!status.paired) return 'Starts when you pair a phone.';
  if (status.holding) return 'On. This computer stays awake while it is plugged in.';
  if (status.onAc === false) return 'Paused while on battery. It stays awake again when you plug it in.';
  return 'Paused: Shodh cannot tell whether this computer is plugged in.';
}
