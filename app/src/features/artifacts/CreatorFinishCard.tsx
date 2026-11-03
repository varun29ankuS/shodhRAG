import React, { useEffect, useState } from 'react';
import { Check, Hammer, LayoutGrid, Pencil, Trash2 } from 'lucide-react';
import { cn } from '../../lib/utils';
import { notify } from '../../lib/notify';
import { publishTarget } from '../agent/navigation';
import { creationsApi, errorText, onCreationsChanged } from './api';
import type { Creation, FinishCard } from './model';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-shodh-surface';
const BUTTON = cn(
  'h-8 px-3 inline-flex items-center gap-1.5 rounded-lg border border-shodh-border text-small font-medium text-shodh-text hover:bg-shodh-raised transition-colors duration-micro disabled:opacity-40',
  FOCUS_RING,
);

interface CreatorFinishCardProps {
  conversationId: string;
  card: FinishCard;
  /** Change something: the composer starts a change request. */
  onChange: () => void;
  /** Sending is not possible right now (an answer runs). */
  disabled: boolean;
}

/**
 * Under a finished Creator answer: what was built and "Here's what I chose" (the agent's
 * choices, shown as plain text and never as actions), then Keep · Change something ·
 * Delete it. Delete asks once more, then removes the creation folder and its versions.
 */
export function CreatorFinishCard({ conversationId, card, onChange, disabled }: CreatorFinishCardProps) {
  const [creation, setCreation] = useState<Creation | null | undefined>(undefined);
  const [confirming, setConfirming] = useState(false);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    let active = true;
    const load = () =>
      creationsApi
        .forConversation(conversationId)
        .then(found => {
          if (active) setCreation(found);
        })
        .catch(() => {
          if (active) setCreation(null);
        });
    void load();
    let unlisten: (() => void) | null = null;
    onCreationsChanged(change => {
      if (change.conversationId === conversationId) void load();
    })
      .then(fn => {
        if (active) unlisten = fn;
        else fn();
      })
      .catch(e => console.error('Failed to listen for creations-changed:', e));
    return () => {
      active = false;
      unlisten?.();
    };
  }, [conversationId]);

  const open = () => {
    if (!creation) return;
    publishTarget({ kind: 'artifact', artifactId: creation.id, version: null, pane: 'preview' });
    window.dispatchEvent(new CustomEvent('switchTab', { detail: 'artifacts' }));
  };

  const keep = async () => {
    if (!creation) return;
    setBusy(true);
    try {
      setCreation(await creationsApi.keep(creation.id));
    } catch (e) {
      notify.error('It could not be kept', { description: errorText(e) });
    } finally {
      setBusy(false);
    }
  };

  const remove = async () => {
    if (!creation) return;
    setBusy(true);
    try {
      await creationsApi.remove(creation.id);
      setCreation(null);
      setConfirming(false);
      notify.success('Deleted', { description: 'The folder and every saved version are gone.' });
    } catch (e) {
      notify.error('It could not be deleted', { description: errorText(e) });
    } finally {
      setBusy(false);
    }
  };

  const gone = creation === null;

  return (
    <section
      aria-label="What was built"
      className="flex flex-col gap-2.5 rounded-xl border border-shodh-border bg-shodh-surface px-4 py-3"
    >
      <p className="m-0 inline-flex items-start gap-2 text-small font-medium text-shodh-text">
        <Hammer className="w-3.5 h-3.5 mt-0.5 shrink-0 text-shodh-text-secondary" aria-hidden="true" />
        {card.summary}
      </p>
      {card.choices.length > 0 && (
        <div className="flex flex-col gap-1">
          <p className="m-0 text-caption font-semibold text-shodh-text-secondary">Here&apos;s what I chose</p>
          <ul className="m-0 pl-5 flex flex-col gap-0.5 text-small text-shodh-text-secondary list-disc">
            {card.choices.map(choice => (
              <li key={choice}>{choice}</li>
            ))}
          </ul>
        </div>
      )}
      {gone ? (
        <p className="m-0 text-caption text-shodh-text-muted">Deleted: its folder and versions are gone.</p>
      ) : confirming && creation ? (
        <div role="alertdialog" aria-label="Delete this creation" className="flex flex-col gap-2">
          <p className="m-0 text-small text-shodh-text">
            Delete its folder ({creation.folder}) and every saved version? This cannot be undone.
          </p>
          <div className="flex items-center gap-2">
            <button
              type="button"
              onClick={() => void remove()}
              disabled={busy || disabled}
              className={cn(BUTTON, 'border-transparent bg-shodh-error text-white hover:bg-shodh-error hover:opacity-90')}
            >
              <Trash2 className="w-3.5 h-3.5" aria-hidden="true" />
              Delete it
            </button>
            <button type="button" onClick={() => setConfirming(false)} disabled={busy} className={BUTTON}>
              Cancel
            </button>
          </div>
        </div>
      ) : (
        <div className="flex items-center gap-2 flex-wrap">
          <button
            type="button"
            onClick={() => void keep()}
            disabled={busy || !creation || creation.status === 'kept'}
            className={BUTTON}
          >
            <Check className="w-3.5 h-3.5" aria-hidden="true" />
            {creation?.status === 'kept' ? 'Kept' : 'Keep'}
          </button>
          <button type="button" onClick={onChange} disabled={disabled} className={BUTTON}>
            <Pencil className="w-3.5 h-3.5" aria-hidden="true" />
            Change something
          </button>
          <button type="button" onClick={() => setConfirming(true)} disabled={busy || disabled || !creation} className={BUTTON}>
            <Trash2 className="w-3.5 h-3.5" aria-hidden="true" />
            Delete it
          </button>
          <button type="button" onClick={open} disabled={!creation} className={cn(BUTTON, 'border-transparent text-shodh-text-secondary')}>
            <LayoutGrid className="w-3.5 h-3.5" aria-hidden="true" />
            Open in Artifacts
          </button>
        </div>
      )}
    </section>
  );
}

export default CreatorFinishCard;
