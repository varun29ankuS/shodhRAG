import { useState } from 'react';
import { ClipboardPaste, Loader2, ScanQrCode, Settings, TriangleAlert } from 'lucide-react';
import { checkPermissions, Format, openAppSettings, requestPermissions, scan } from '@tauri-apps/plugin-barcode-scanner';
import { errorText, pair } from '../lib/link';

/** Pair with the computer: scan its code, or paste it. */
export function Pair({ refusal }: { refusal: string | null }) {
  const [code, setCode] = useState('');
  const [busy, setBusy] = useState<'scan' | 'pair' | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [cameraDenied, setCameraDenied] = useState(false);

  const pairWith = async (text: string) => {
    setBusy('pair');
    setError(null);
    try {
      await pair(text.trim());
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(null);
    }
  };

  const scanCode = async () => {
    setBusy('scan');
    setError(null);
    try {
      let permission = await checkPermissions();
      if (permission !== 'granted') permission = await requestPermissions();
      if (permission !== 'granted') {
        setCameraDenied(true);
        setBusy(null);
        return;
      }
      setCameraDenied(false);
      const scanned = await scan({ formats: [Format.QRCode], windowed: false });
      await pairWith(scanned.content);
    } catch (e) {
      setError(errorText(e));
      setBusy(null);
    }
  };

  return (
    <section className="screen">
      <h1>Pair with your computer</h1>
      <p className="muted">
        On your computer, open Shodh, go to Settings, then Phone, and choose Pair a phone. Scan the code it shows. Your
        phone then talks to your computer directly, end to end encrypted.
      </p>
      {refusal && (
        <div className="callout callout-error" role="alert">
          <TriangleAlert aria-hidden="true" className="icon" />
          <p>{refusal} Pair again to use this phone.</p>
        </div>
      )}
      <button type="button" className="button button-primary" disabled={busy !== null} onClick={scanCode}>
        {busy === 'scan' ? <Loader2 aria-hidden="true" className="icon spin" /> : <ScanQrCode aria-hidden="true" className="icon" />}
        Scan the code
      </button>
      {cameraDenied && (
        <div className="callout">
          <p>Shodh needs the camera to scan the code. Allow it in the app's settings, or paste the code below.</p>
          <button type="button" className="button" onClick={() => void openAppSettings()}>
            <Settings aria-hidden="true" className="icon" />
            Open app settings
          </button>
        </div>
      )}
      <form
        className="paste"
        onSubmit={event => {
          event.preventDefault();
          if (code.trim()) void pairWith(code);
        }}
      >
        <label htmlFor="pair-code">Or paste the code</label>
        <textarea
          id="pair-code"
          rows={3}
          value={code}
          onChange={event => setCode(event.target.value)}
          placeholder="shodh-pair:1:…"
          autoCapitalize="off"
          autoCorrect="off"
          spellCheck={false}
        />
        <button type="submit" className="button" disabled={busy !== null || code.trim() === ''}>
          {busy === 'pair' ? <Loader2 aria-hidden="true" className="icon spin" /> : <ClipboardPaste aria-hidden="true" className="icon" />}
          {busy === 'pair' ? 'Pairing…' : 'Pair'}
        </button>
      </form>
      {error && (
        <div className="callout callout-error" role="alert">
          <TriangleAlert aria-hidden="true" className="icon" />
          <p>{error}</p>
        </div>
      )}
    </section>
  );
}
