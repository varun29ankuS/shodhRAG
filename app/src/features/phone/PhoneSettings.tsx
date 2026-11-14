import React, { useCallback, useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { Loader2, QrCode as QrIcon, RefreshCw, Smartphone, Trash2 } from 'lucide-react';
import { QrCode } from '../connectors/TelegramLink';
import { countdown } from '../connectors/telegramModel';
import { Callout, DANGER_BUTTON, OUTLINE_BUTTON, PRIMARY_BUTTON, QUIET_BUTTON, Skeleton } from '../connectors/ui';
import { errorText, type PairedPhone, type PairingCode, PHONE_DEVICES_CHANGED, whenSeen } from './phoneModel';

/**
 * Settings → Phone: pair a phone with a QR code (it works once, for five minutes), and
 * the paired phones, each with Remove (it is locked out at once). The code itself is
 * never shown as text, only as the QR code to scan.
 */
export default function PhoneSettings() {
  const [phones, setPhones] = useState<PairedPhone[] | null>(null);
  const [listError, setListError] = useState<string | null>(null);
  const [code, setCode] = useState<PairingCode | null>(null);
  const [pairing, setPairing] = useState(false);
  const [pairError, setPairError] = useState<string | null>(null);
  const [confirming, setConfirming] = useState<string | null>(null);
  const [removing, setRemoving] = useState<string | null>(null);
  const [now, setNow] = useState(() => Date.now());

  const load = useCallback(() => {
    invoke<PairedPhone[]>('phone_devices')
      .then(list => { setPhones(list); setListError(null); })
      .catch(err => setListError(errorText(err)));
  }, []);

  const pair = useCallback(() => {
    setPairing(true);
    setPairError(null);
    invoke<PairingCode>('phone_pairing_start')
      .then(next => { setCode(next); setNow(Date.now()); })
      .catch(err => setPairError(errorText(err)))
      .finally(() => setPairing(false));
  }, []);

  useEffect(() => { load(); }, [load]);

  // A phone that pairs (or one removed elsewhere) updates the list; the code it used is done.
  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | null = null;
    listen(PHONE_DEVICES_CHANGED, () => { setCode(null); load(); })
      .then(fn => { if (disposed) fn(); else unlisten = fn; })
      .catch(err => setListError(errorText(err)));
    return () => { disposed = true; unlisten?.(); };
  }, [load]);

  // With no phone paired yet, the code is shown at once.
  useEffect(() => {
    if (phones !== null && phones.length === 0 && !code && !pairing && !pairError) pair();
  }, [phones, code, pairing, pairError, pair]);

  const left = code ? countdown(code.expiresAt, now) : null;
  useEffect(() => {
    if (!code || left === null) return;
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [code, left === null]);

  const remove = (nodeId: string) => {
    setRemoving(nodeId);
    invoke<boolean>('phone_device_remove', { nodeId })
      .then(() => load())
      .catch(err => setListError(errorText(err)))
      .finally(() => { setRemoving(null); setConfirming(null); });
  };

  const expired = code !== null && left === null;

  return (
    <div className="flex flex-col gap-6">
      <div className="flex flex-col sm:flex-row gap-5">
        <div className="shrink-0 self-center sm:self-start">
          {code && !expired ? (
            <QrCode rows={code.qr.rows} label="QR code that pairs your phone with Shodh on this computer" className="w-48 h-48 border border-black/10" />
          ) : (
            <div className="w-48 h-48 rounded-lg border border-dashed border-shodh-border flex items-center justify-center p-4 text-center">
              {pairing ? (
                <Skeleton className="w-full h-full" />
              ) : (
                <span className="text-caption text-shodh-text-muted">{expired ? 'This code expired.' : 'No code shown.'}</span>
              )}
            </div>
          )}
        </div>

        <div className="min-w-0 flex-1 flex flex-col gap-3">
          <div>
            <p className="m-0 text-body font-semibold text-shodh-text">Scan with Shodh on your phone</p>
            <p className="m-0 mt-0.5 text-small text-shodh-text-secondary max-w-[56ch]">
              Open Shodh on your Android phone and choose Pair with computer. The phone then asks this computer
              directly, end to end encrypted. Phones can ask and read; anything in Code mode, file changes and
              commands still need you here.
            </p>
          </div>
          <div className="flex flex-wrap items-center gap-2">
            <button type="button" className={code && !expired ? OUTLINE_BUTTON : PRIMARY_BUTTON} disabled={pairing} onClick={pair}>
              {pairing ? (
                <Loader2 className="w-4 h-4 animate-spin motion-reduce:animate-none" aria-hidden="true" />
              ) : code ? (
                <RefreshCw className="w-4 h-4" aria-hidden="true" />
              ) : (
                <QrIcon className="w-4 h-4" aria-hidden="true" />
              )}
              {code ? 'New code' : 'Pair a phone'}
            </button>
          </div>
          <p className="m-0 text-caption text-shodh-text-muted" aria-live="off">
            {code && left ? (
              <>Works once, for <span className="tabular-nums">{left}</span> more. A new code replaces this one.</>
            ) : (
              'A code works once, for five minutes.'
            )}
          </p>
          {pairError && (
            <Callout tone="error" role="alert">
              <p className="m-0">The code could not be made: {pairError}</p>
            </Callout>
          )}
        </div>
      </div>

      <div className="flex flex-col gap-2">
        <h3 className="m-0 text-body font-semibold text-shodh-text">Paired phones</h3>
        {phones === null && !listError ? (
          <Skeleton className="h-12 w-full" />
        ) : phones && phones.length === 0 ? (
          <p className="m-0 text-small text-shodh-text-muted">No phone is paired yet.</p>
        ) : (
          <ul className="m-0 p-0 list-none flex flex-col gap-2">
            {(phones ?? []).map(phone => (
              <li key={phone.nodeId} className="flex items-center gap-3 rounded-lg border border-shodh-border bg-shodh-surface px-3.5 py-2.5">
                <Smartphone className="w-4 h-4 shrink-0 text-shodh-text-muted" aria-hidden="true" />
                <div className="min-w-0 flex-1">
                  <p className="m-0 text-body text-shodh-text truncate">{phone.name}</p>
                  <p className="m-0 text-caption text-shodh-text-muted">{whenSeen(phone)}</p>
                </div>
                {confirming === phone.nodeId ? (
                  <div className="flex items-center gap-2">
                    <span className="text-small text-shodh-text-secondary">Remove it? It is locked out at once.</span>
                    <button type="button" className={DANGER_BUTTON} disabled={removing !== null} onClick={() => remove(phone.nodeId)}>
                      {removing === phone.nodeId ? (
                        <Loader2 className="w-3.5 h-3.5 animate-spin motion-reduce:animate-none" aria-hidden="true" />
                      ) : (
                        <Trash2 className="w-3.5 h-3.5" aria-hidden="true" />
                      )}
                      Remove
                    </button>
                    <button type="button" className={QUIET_BUTTON} disabled={removing !== null} onClick={() => setConfirming(null)}>
                      Keep
                    </button>
                  </div>
                ) : (
                  <button type="button" className={DANGER_BUTTON} onClick={() => setConfirming(phone.nodeId)}>
                    <Trash2 className="w-3.5 h-3.5" aria-hidden="true" />
                    Remove
                  </button>
                )}
              </li>
            ))}
          </ul>
        )}
        {listError && (
          <Callout tone="error" role="alert">
            <p className="m-0">The paired phones could not be read: {listError}</p>
          </Callout>
        )}
      </div>
    </div>
  );
}
