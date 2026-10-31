import React, { useEffect, useId, useState } from 'react';
import * as Dialog from '@radix-ui/react-dialog';
import { Loader2, X } from 'lucide-react';
import { cn } from '../../lib/utils';
import { tableError, tablesApi } from './api';
import { reasonText } from './format';
import {
  cleanSpec,
  KIND_LABELS,
  MAX_INSTRUCTIONS_CHARS,
  MAX_OPTIONS,
  MAX_QUESTION_CHARS,
  MIN_OPTIONS,
  specProblem,
} from './model';
import type { Column, ColumnKind, ColumnSpec } from './types';
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
  TEXTAREA,
} from './ui';

const KINDS: readonly ColumnKind[] = ['text', 'number', 'yes_no', 'choice'];

const KIND_HELP: Record<ColumnKind, string> = {
  text: 'A short answer in words (up to 500 characters).',
  number: 'One number as the paper prints it, with its unit; never computed or converted.',
  yes_no: 'Yes or no.',
  choice: `One of your options (${MIN_OPTIONS} to ${MAX_OPTIONS}).`,
};

/**
 * A column: its question, the kind of answer, the options of a pick-list, and
 * instructions for the model. Editing the question, kind, options or instructions asks
 * the column again for every paper on the next fill; edited cells keep your values.
 */
export function ColumnDialog({
  open,
  onOpenChange,
  tableId,
  column,
  onSaved,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  tableId: string;
  /** The column to edit; null adds one. */
  column: Column | null;
  onSaved: () => void;
}) {
  const ids = { question: useId(), kind: useId(), options: useId(), optionsHelp: useId(), instructions: useId(), desc: useId() };
  const [question, setQuestion] = useState('');
  const [kind, setKind] = useState<ColumnKind>('text');
  const [options, setOptions] = useState('');
  const [instructions, setInstructions] = useState('');
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!open) return;
    setQuestion(column?.question ?? '');
    setKind(column?.kind ?? 'text');
    setOptions((column?.options ?? []).join('\n'));
    setInstructions(column?.instructions ?? '');
    setError(null);
  }, [open, column]);

  const spec: ColumnSpec = { question, kind, options: options.split('\n'), instructions };
  const problem = specProblem(spec);

  const save = async () => {
    if (problem) {
      setError(problem);
      return;
    }
    setSaving(true);
    setError(null);
    try {
      const clean = cleanSpec(spec);
      if (column) await tablesApi.updateColumn(tableId, column.id, clean);
      else await tablesApi.addColumn(tableId, clean);
      onSaved();
      onOpenChange(false);
    } catch (err) {
      const failure = tableError(err);
      setError(failure.message || reasonText(failure.code));
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
              <Dialog.Title className="text-body-lg font-semibold">{column ? 'Edit column' : 'New column'}</Dialog.Title>
              <Dialog.Description id={ids.desc} className="text-small text-shodh-text-muted">
                {column
                  ? 'Changing the question, kind, options or instructions asks this column again for every paper on the next fill. Cells you edited keep your values.'
                  : 'A question asked of every paper in the table. Each answer comes with the quote it rests on.'}
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
              void save();
            }}
          >
            <div className="flex flex-col gap-1">
              <label htmlFor={ids.question} className={LABEL}>Question</label>
              <textarea
                id={ids.question}
                className={TEXTAREA}
                rows={2}
                maxLength={MAX_QUESTION_CHARS}
                value={question}
                onChange={e => setQuestion(e.target.value)}
                placeholder="e.g. Which datasets are used in the experiments?"
                autoFocus
                required
              />
            </div>
            <fieldset className="flex flex-col gap-2 m-0 p-0 border-0">
              <legend className={cn(LABEL, 'mb-1.5')}>Answer</legend>
              <div className="grid grid-cols-2 gap-2">
                {KINDS.map(k => (
                  <label
                    key={k}
                    className={cn(
                      'flex items-start gap-2 p-2.5 rounded-lg border cursor-pointer transition-colors duration-micro',
                      kind === k ? 'border-shodh-accent bg-shodh-accent-soft' : 'border-shodh-border hover:bg-shodh-raised',
                    )}
                  >
                    <input
                      type="radio"
                      name={ids.kind}
                      value={k}
                      checked={kind === k}
                      onChange={() => setKind(k)}
                      className={cn('mt-0.5 accent-[var(--c-accent)]', FOCUS_RING)}
                    />
                    <span className="min-w-0">
                      <span className="block text-small font-semibold text-shodh-text">{KIND_LABELS[k]}</span>
                      <span className="block text-caption text-shodh-text-muted">{KIND_HELP[k]}</span>
                    </span>
                  </label>
                ))}
              </div>
            </fieldset>
            {kind === 'choice' && (
              <div className="flex flex-col gap-1">
                <label htmlFor={ids.options} className={LABEL}>Options, one per line</label>
                <textarea
                  id={ids.options}
                  className={TEXTAREA}
                  rows={4}
                  value={options}
                  onChange={e => setOptions(e.target.value)}
                  aria-describedby={ids.optionsHelp}
                  placeholder={'e.g.\nGraph-based\nQuantization\nHashing'}
                />
                <p id={ids.optionsHelp} className="m-0 text-caption text-shodh-text-muted">
                  {MIN_OPTIONS} to {MAX_OPTIONS} different options. The model must answer with one of them.
                </p>
              </div>
            )}
            <div className="flex flex-col gap-1">
              <label htmlFor={ids.instructions} className={LABEL}>Instructions for the model (optional)</label>
              <textarea
                id={ids.instructions}
                className={TEXTAREA}
                rows={2}
                maxLength={MAX_INSTRUCTIONS_CHARS}
                value={instructions}
                onChange={e => setInstructions(e.target.value)}
                placeholder="e.g. Report the best result on the main benchmark."
              />
            </div>
            {error && <p role="alert" className="m-0 text-small text-shodh-error">{error}</p>}
            <div className="flex items-center justify-end gap-2 pt-1">
              <Dialog.Close className={QUIET_BUTTON} type="button">Cancel</Dialog.Close>
              <button type="submit" className={PRIMARY_BUTTON} disabled={saving || question.trim().length === 0}>
                {saving && <Loader2 className="w-4 h-4 animate-spin motion-reduce:animate-none" aria-hidden="true" />}
                {column ? 'Save column' : 'Add column'}
              </button>
            </div>
          </form>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
