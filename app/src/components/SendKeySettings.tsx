import { useId } from 'react';
import { cn } from '../lib/utils';
import { IS_MAC_PLATFORM, shortcutText } from '../lib/shortcuts';
import { SEND_KEYS, sendShortcut } from '../lib/sendKey';
import type { SendKey } from '../lib/sendKey';
import { useSendKey } from '../hooks/useSendKey';
import { tr } from '../i18n/tr';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-shodh-surface';

/** "Send with Enter" or "Send with Ctrl+Enter" (⌘ on a Mac), from the shortcut registry. */
function optionLabel(key: SendKey): string {
  return `Send with ${shortcutText(sendShortcut(key), IS_MAC_PLATFORM)}`;
}

/** Settings → General → Send: which key sends from the composer. */
export default function SendKeySettings() {
  const { sendKey, setSendKey } = useSendKey();
  const labelId = useId();
  return (
    <div className="flex flex-col gap-3" data-testid="settings-send-key">
      <p id={labelId} className="m-0 text-body font-semibold text-shodh-text">{tr('settings.sendKeySettings.send')}</p>
      <div
        role="radiogroup"
        aria-labelledby={labelId}
        className="inline-flex w-fit flex-wrap gap-1 p-1 rounded-xl bg-shodh-raised-2"
        onKeyDown={e => {
          const step = e.key === 'ArrowRight' || e.key === 'ArrowDown' ? 1 : e.key === 'ArrowLeft' || e.key === 'ArrowUp' ? -1 : 0;
          if (step === 0) return;
          e.preventDefault();
          const i = SEND_KEYS.indexOf(sendKey);
          const next = SEND_KEYS[(i + step + SEND_KEYS.length) % SEND_KEYS.length];
          setSendKey(next);
          e.currentTarget.querySelector<HTMLButtonElement>(`[data-send-key="${next}"]`)?.focus();
        }}
      >
        {SEND_KEYS.map(option => {
          const selected = option === sendKey;
          return (
            <button
              key={option}
              type="button"
              role="radio"
              aria-checked={selected}
              data-send-key={option}
              tabIndex={selected ? 0 : -1}
              title={option === 'enter' ? 'Shift+Enter: new line' : 'Enter: new line'}
              onClick={() => { if (!selected) setSendKey(option); }}
              className={cn(
                'h-8 px-3.5 rounded-lg text-small font-medium whitespace-nowrap transition-colors duration-micro',
                selected ? 'bg-shodh-surface text-shodh-text shadow-sm' : 'text-shodh-text-muted hover:text-shodh-text',
                FOCUS_RING,
              )}
            >
              {optionLabel(option)}
            </button>
          );
        })}
      </div>
    </div>
  );
}
