import React, { useEffect, useId, useState } from 'react';
import * as Dialog from '@radix-ui/react-dialog';
import { Loader2, X } from 'lucide-react';
import { cn } from '../../lib/utils';
import { notify } from '../../lib/notify';
import { tableError, tablesApi } from './api';
import { reasonText } from './format';
import { KIND_LABELS, MAX_NAME_CHARS, PRESET_COLUMNS } from './model';
import type { EvidenceTable } from './types';
import {
  CLOSE_BUTTON,
  DIALOG_BODY,
  DIALOG_CONTENT,
  DIALOG_HEADER,
  DIALOG_OVERLAY,
  FOCUS_RING,
  INPUT,
  LABEL,
  PRIMARY_BUTTON,
  QUIET_BUTTON,
} from './ui';

/** Presets ticked for a new table: the row already shows each paper's title. */
const DEFAULT_PRESETS: readonly string[] = ['year', 'method', 'dataset', 'main-result', 'limitations'];

/** New table: a name and the preset columns to start with (each can be edited later). */
export function NewTableDialog({
  open,
  onOpenChange,
  workspaceId,
  onCreated,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  workspaceId: string;
  onCreated: (table: EvidenceTable) => void;
}) {
  const ids = { name: useId(), desc: useId(), presets: useId() };
  const [name, setName] = useState('');
  const [presets, setPresets] = useState<ReadonlySet<string>>(() => new Set(DEFAULT_PRESETS));
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!open) return;
    setName('');
    setPresets(new Set(DEFAULT_PRESETS));
    setError(null);
  }, [open]);

  const create = async () => {
    const trimmed = name.trim();
    if (!trimmed || saving) return;
    setSaving(true);
    setError(null);
    let table: EvidenceTable | null = null;
    try {
      table = await tablesApi.create(workspaceId, trimmed);
      for (const preset of PRESET_COLUMNS) {
        if (presets.has(preset.id)) await tablesApi.addColumn(table.id, preset.spec);
      }
      onCreated(table);
      onOpenChange(false);
    } catch (err) {
      const failure = tableError(err);
      const words = failure.message || reasonText(failure.code);
      if (table) {
        // The table exists: open it, where the missing columns can be added.
        notify.error('Not every column was added', { description: words });
        onCreated(table);
        onOpenChange(false);
      } else {
        setError(words);
      }
    } finally {
      setSaving(false);
    }
  };

  return (
    <Dialog.Root open={open} onOpenChange={onOpenChange}>
      <Dialog.Portal>
        <Dialog.Overlay className={DIALOG_OVERLAY} />
        <Dialog.Content aria-describedby={ids.desc} className={DIALOG_CONTENT}>
          <header className={DIALOG_HEADER}>
            <div className="flex-1 min-w-0">
              <Dialog.Title className="text-body-lg font-semibold">New evidence table</Dialog.Title>
              <Dialog.Description id={ids.desc} className="text-small text-shodh-text-muted">
                One row per paper, one column per question. Every answer carries the quote it rests on, checked word for
                word against the paper, and its page.
              </Dialog.Description>
            </div>
            <Dialog.Close aria-label="Close" className={cn(CLOSE_BUTTON, FOCUS_RING)}>
              <X className="w-4 h-4" aria-hidden="true" />
            </Dialog.Close>
          </header>
          <form
            className={DIALOG_BODY}
            onSubmit={e => {
              e.preventDefault();
              void create();
            }}
          >
            <div className="flex flex-col gap-1">
              <label htmlFor={ids.name} className={LABEL}>Name</label>
              <input
                id={ids.name}
                className={INPUT}
                value={name}
                maxLength={MAX_NAME_CHARS}
                onChange={e => setName(e.target.value)}
                placeholder="e.g. ANN indexes compared"
                autoFocus
                required
              />
            </div>
            <fieldset className="flex flex-col gap-1.5 m-0 p-0 border-0">
              <legend id={ids.presets} className={cn(LABEL, 'mb-1.5')}>Start with these columns</legend>
              {PRESET_COLUMNS.map(preset => (
                <label key={preset.id} className="flex items-start gap-2.5 py-1 px-1 rounded-md hover:bg-shodh-raised/60 cursor-pointer">
                  <input
                    type="checkbox"
                    checked={presets.has(preset.id)}
                    onChange={e =>
                      setPresets(prev => {
                        const next = new Set(prev);
                        if (e.target.checked) next.add(preset.id);
                        else next.delete(preset.id);
                        return next;
                      })
                    }
                    className={cn('mt-0.5 w-3.5 h-3.5 accent-[var(--c-accent)]', FOCUS_RING)}
                  />
                  <span className="min-w-0">
                    <span className="block text-small font-semibold text-shodh-text">
                      {preset.label} <span className="font-normal text-shodh-text-muted">· {KIND_LABELS[preset.spec.kind]}</span>
                    </span>
                    <span className="block text-caption text-shodh-text-muted">{preset.spec.question}</span>
                  </span>
                </label>
              ))}
            </fieldset>
            {error && <p role="alert" className="m-0 text-small text-shodh-error">{error}</p>}
            <div className="flex items-center justify-end gap-2 pt-1">
              <Dialog.Close className={QUIET_BUTTON} type="button">Cancel</Dialog.Close>
              <button type="submit" className={PRIMARY_BUTTON} disabled={saving || name.trim().length === 0}>
                {saving && <Loader2 className="w-4 h-4 animate-spin motion-reduce:animate-none" aria-hidden="true" />}
                Create table
              </button>
            </div>
          </form>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
