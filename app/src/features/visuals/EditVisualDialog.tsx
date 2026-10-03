import React, { useEffect, useId, useState } from 'react';
import * as Dialog from '@radix-ui/react-dialog';
import { X } from 'lucide-react';
import { cn } from '../../lib/utils';
import { notify } from '../../lib/notify';
import { toVisualError, visualsApi } from './api';
import { MAX_VISUAL_TITLE_CHARS } from './extract';
import type { VisualDetail, VisualRecord } from './model';

/** Longest note (the backend's cap). */
const MAX_NOTE_CHARS = 2_000;

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-shodh-surface';
const FIELD = cn(
  'w-full rounded-lg border border-shodh-border bg-shodh-surface-2 px-3 py-2 text-[13.5px] text-shodh-text placeholder:text-shodh-text-faint',
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring',
);

/**
 * Rename a visual and edit its note (both apply to every version). Saves only
 * what changed.
 */
export function EditVisualDialog({
  record,
  open,
  onOpenChange,
  onSaved,
}: {
  record: Pick<VisualRecord, 'id' | 'title' | 'note'>;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onSaved?: (detail: VisualDetail) => void;
}) {
  const titleId = useId();
  const noteId = useId();
  const [title, setTitle] = useState(record.title);
  const [note, setNote] = useState(record.note);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!open) return;
    setTitle(record.title);
    setNote(record.note);
    setError(null);
  }, [open, record.title, record.note]);

  const save = async (e: React.FormEvent) => {
    e.preventDefault();
    const nextTitle = title.trim();
    if (!nextTitle) {
      setError('A title cannot be empty.');
      return;
    }
    setSaving(true);
    setError(null);
    try {
      let detail: VisualDetail | null = null;
      if (nextTitle !== record.title) detail = await visualsApi.rename(record.id, nextTitle);
      if (note.trim() !== record.note) detail = await visualsApi.setNote(record.id, note);
      if (detail) {
        onSaved?.(detail);
        notify.success('Visual updated');
      }
      onOpenChange(false);
    } catch (err) {
      setError(toVisualError(err).message);
    } finally {
      setSaving(false);
    }
  };

  return (
    <Dialog.Root open={open} onOpenChange={onOpenChange}>
      <Dialog.Portal>
        <Dialog.Overlay className="ask-fade-in fixed inset-0 z-[70] bg-black/40" />
        <Dialog.Content
          className="ask-fade-in fixed z-[70] left-1/2 top-1/2 -translate-x-1/2 -translate-y-1/2 w-[min(460px,calc(100vw-32px))] rounded-[16px] border border-shodh-border-strong bg-shodh-surface p-5 text-shodh-text shadow-[0_24px_80px_rgba(0,0,0,0.45)] focus:outline-none"
        >
          <div className="flex items-start justify-between gap-3">
            <Dialog.Title className="text-[15px] font-semibold">Rename and note</Dialog.Title>
            <Dialog.Close
              aria-label="Close"
              className={cn('w-7 h-7 -mt-1 -mr-1 inline-flex items-center justify-center rounded-lg text-shodh-text-muted hover:bg-shodh-raised hover:text-shodh-text', FOCUS_RING)}
            >
              <X className="w-4 h-4" aria-hidden="true" />
            </Dialog.Close>
          </div>
          <Dialog.Description className="mt-1 text-[12.5px] text-shodh-text-muted">
            Applies to every version of this visual. The note is searchable in the gallery.
          </Dialog.Description>
          <form onSubmit={e => void save(e)} className="mt-4 flex flex-col gap-3">
            <div className="flex flex-col gap-1.5">
              <label htmlFor={titleId} className="text-[12.5px] font-medium text-shodh-text-secondary">Title</label>
              <input
                id={titleId}
                value={title}
                maxLength={MAX_VISUAL_TITLE_CHARS}
                onChange={e => setTitle(e.target.value)}
                className={FIELD}
                autoFocus
              />
            </div>
            <div className="flex flex-col gap-1.5">
              <label htmlFor={noteId} className="text-[12.5px] font-medium text-shodh-text-secondary">Note</label>
              <textarea
                id={noteId}
                value={note}
                maxLength={MAX_NOTE_CHARS}
                rows={4}
                onChange={e => setNote(e.target.value)}
                placeholder="Why you kept it, where you used it…"
                className={cn(FIELD, 'resize-y min-h-[84px]')}
              />
            </div>
            {error && <p role="alert" className="text-[12.5px] text-shodh-error">{error}</p>}
            <div className="flex justify-end gap-2 pt-1">
              <Dialog.Close
                type="button"
                className={cn('h-8 px-3 rounded-lg text-[12.5px] text-shodh-text-secondary hover:bg-shodh-raised hover:text-shodh-text', FOCUS_RING)}
              >
                Cancel
              </Dialog.Close>
              <button
                type="submit"
                disabled={saving}
                className={cn('h-8 px-3.5 rounded-lg bg-shodh-accent text-shodh-on-accent text-[12.5px] font-semibold hover:bg-shodh-accent-hover disabled:opacity-50', FOCUS_RING)}
              >
                {saving ? 'Saving…' : 'Save'}
              </button>
            </div>
          </form>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
