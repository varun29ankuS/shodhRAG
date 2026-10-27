import React, { useCallback, useEffect, useState } from 'react';
import { openUrl } from '@tauri-apps/plugin-opener';
import { Check, Copy, ExternalLink, Loader2, RefreshCw } from 'lucide-react';
import { cn } from '../../lib/utils';
import { notify } from '../../lib/notify';
import { slackError } from './api';
import { telegramApi } from './telegramApi';
import { countdown, qrPath, type LinkInvite } from './telegramModel';
import { Callout, OUTLINE_BUTTON, PRIMARY_BUTTON, QUIET_BUTTON, Skeleton } from './ui';

/** A QR code drawn from its modules: crisp at any size, black on white in both themes. */
export function QrCode({ rows, label, className }: { rows: string[]; label: string; className?: string }) {
  const { path, size } = qrPath(rows);
  return (
    <svg
      role="img"
      aria-label={label}
      viewBox={`0 0 ${size} ${size}`}
      shapeRendering="crispEdges"
      className={cn('block bg-white rounded-lg', className)}
    >
      <path d={path} fill="#000000" />
    </svg>
  );
}

/**
 * A one-time link that links a Telegram account: its QR code to scan with the phone, the
 * link to open on this computer or copy, and how long it still works. A new link replaces
 * the previous one.
 */
export function LinkCard({
  kind,
  botUsername,
  disabled = false,
}: {
  kind: 'owner' | 'invite';
  botUsername: string;
  disabled?: boolean;
}) {
  const [invite, setInvite] = useState<LinkInvite | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [copied, setCopied] = useState(false);
  const [now, setNow] = useState(() => Date.now());

  const make = useCallback(() => {
    setLoading(true);
    setError(null);
    telegramApi
      .linkCode(kind)
      .then(next => { setInvite(next); setNow(Date.now()); })
      .catch(err => setError(slackError(err)))
      .finally(() => setLoading(false));
  }, [kind]);

  useEffect(() => {
    if (!disabled) make();
  }, [make, disabled]);

  const left = invite ? countdown(invite.expiresAt, now) : null;
  useEffect(() => {
    if (!invite || left === null) return;
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [invite, left === null]);

  useEffect(() => {
    if (!copied) return;
    const timer = window.setTimeout(() => setCopied(false), 2000);
    return () => window.clearTimeout(timer);
  }, [copied]);

  const copy = async () => {
    if (!invite) return;
    try {
      await navigator.clipboard.writeText(invite.url);
      setCopied(true);
    } catch (err) {
      notify.error('The link could not be copied', { description: slackError(err) });
    }
  };

  const open = () => {
    if (!invite) return;
    void openUrl(invite.url).catch(err => notify.error('Telegram could not be opened', { description: slackError(err) }));
  };

  const expired = invite !== null && left === null;
  const who = kind === 'owner' ? 'your account' : 'the person you invite';

  return (
    <div className="flex flex-col sm:flex-row gap-5 rounded-xl border border-shodh-border bg-shodh-surface p-5">
      <div className="shrink-0 self-center sm:self-start">
        {invite && !expired ? (
          <QrCode
            rows={invite.qr.rows}
            label={`QR code that links ${who} to @${botUsername}`}
            className="w-44 h-44 border border-black/10"
          />
        ) : (
          <div className="w-44 h-44 rounded-lg border border-dashed border-shodh-border flex items-center justify-center p-4 text-center">
            {loading ? (
              <Skeleton className="w-full h-full" />
            ) : (
              <span className="text-[12px] text-shodh-text-muted">{expired ? 'This code expired.' : 'No code yet.'}</span>
            )}
          </div>
        )}
      </div>

      <div className="min-w-0 flex-1 flex flex-col gap-3">
        <div>
          <p className="m-0 text-[13.5px] font-semibold text-shodh-text">
            {kind === 'owner' ? 'Scan with your phone’s camera' : 'Send this link to one person'}
          </p>
          <p className="m-0 mt-0.5 text-[12.5px] text-shodh-text-secondary max-w-[56ch]">
            {kind === 'owner'
              ? `It opens your chat with @${botUsername} in Telegram. Press Start, and that chat is your Shodh.`
              : `Whoever opens it first and presses Start can ask questions of the workspace you chose. It works once.`}
          </p>
        </div>

        {invite && !expired && (
          <p className="m-0 font-mono text-[12px] text-shodh-text-muted break-all select-all">{invite.url}</p>
        )}

        <div className="flex flex-wrap items-center gap-2">
          {kind === 'owner' && (
            <button type="button" className={PRIMARY_BUTTON} disabled={!invite || expired || disabled} onClick={open}>
              <ExternalLink className="w-4 h-4" aria-hidden="true" /> Open in Telegram
            </button>
          )}
          <button type="button" className={kind === 'owner' ? OUTLINE_BUTTON : PRIMARY_BUTTON} disabled={!invite || expired || disabled} onClick={() => void copy()}>
            {copied ? <Check className="w-3.5 h-3.5 text-shodh-success" aria-hidden="true" /> : <Copy className="w-3.5 h-3.5" aria-hidden="true" />}
            {copied ? 'Link copied' : 'Copy link'}
          </button>
          <button type="button" className={QUIET_BUTTON} disabled={loading || disabled} onClick={make}>
            {loading ? <Loader2 className="w-3.5 h-3.5 animate-spin motion-reduce:animate-none" aria-hidden="true" /> : <RefreshCw className="w-3.5 h-3.5" aria-hidden="true" />}
            New link
          </button>
          <span aria-live="polite" className="sr-only">{copied ? 'The link was copied.' : ''}</span>
        </div>

        <p className="m-0 text-[12px] text-shodh-text-muted" aria-live="off">
          {invite && left ? (
            <>Works once, for <span className="tabular-nums">{left}</span> more. A new link replaces this one.</>
          ) : expired ? (
            'Links work for ten minutes. Make a new one.'
          ) : (
            'Links work once, for ten minutes.'
          )}
        </p>

        {error && (
          <Callout tone="error" role="alert">
            <p className="m-0">The link could not be made: {error}</p>
          </Callout>
        )}
      </div>
    </div>
  );
}
