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
