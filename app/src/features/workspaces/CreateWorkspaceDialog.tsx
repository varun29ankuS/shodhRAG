import React, { useEffect, useId, useState } from 'react';
import * as Dialog from '@radix-ui/react-dialog';
import { Check, Loader2, X } from 'lucide-react';
import { cn } from '../../lib/utils';
import { notify } from '../../lib/notify';
import { workspaceError, workspacesApi } from './api';
import { orderTemplates } from './model';
import type { Workspace, WorkspaceTemplate } from './types';
import { WorkspaceIcon } from './WorkspaceIcon';
import { FOCUS_RING, INPUT, LABEL, PRIMARY_BUTTON, QUIET_BUTTON, TEXTAREA } from './ui';

/**
 * New workspace: a template (or blank), a name and a description. The template's
 * instructions are a starting point the user edits on the workspace's Overview.
 */
export function CreateWorkspaceDialog({
  open,
  onOpenChange,
  onCreated,
  initialTemplate = null,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onCreated: (workspace: Workspace) => void;
  initialTemplate?: string | null;
}) {
  const ids = { name: useId(), description: useId(), templates: useId(), desc: useId() };
  const [templates, setTemplates] = useState<WorkspaceTemplate[] | null>(null);
  const [template, setTemplate] = useState<string>(initialTemplate ?? 'blank');
  const [name, setName] = useState('');
  const [description, setDescription] = useState('');
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!open) return;
    setTemplate(initialTemplate ?? 'blank');
    setName('');
    setDescription('');
    setError(null);
    let cancelled = false;
    workspacesApi
      .templates()
      .then(list => { if (!cancelled) setTemplates(orderTemplates(list)); })
      .catch(err => { if (!cancelled) setError(workspaceError(err).message); });
    return () => { cancelled = true; };
  }, [open, initialTemplate]);

  const chosen = templates?.find(t => t.id === template) ?? null;
  const canCreate = name.trim().length > 0 && !saving;

  const create = async () => {
    if (!canCreate) return;
    setSaving(true);
    setError(null);
    try {
      const created = await workspacesApi.create({ name: name.trim(), description: description.trim(), template });
      notify.success(`Created “${created.name}”`, { description: 'Add its sources next: answers in it search only them.' });
      onCreated(created);
      onOpenChange(false);
    } catch (err) {
      setError(workspaceError(err).message);
    } finally {
      setSaving(false);
    }
  };

  return (
    <Dialog.Root open={open} onOpenChange={onOpenChange}>
      <Dialog.Portal>
        <Dialog.Overlay className="ask-fade-in fixed inset-0 z-[55] bg-black/40" />
        <Dialog.Content
          aria-describedby={ids.desc}
          className="ask-fade-in fixed z-[55] inset-0 m-auto flex flex-col w-[min(720px,calc(100vw-48px))] max-h-[min(760px,calc(100vh-48px))] h-fit rounded-[18px] border border-shodh-border-strong bg-shodh-surface text-shodh-text shadow-[0_24px_80px_rgba(0,0,0,0.45)] focus:outline-none"
        >
          <header className="shrink-0 flex items-start gap-3 px-5 py-4 border-b border-shodh-border-subtle">
            <div className="flex-1 min-w-0">
              <Dialog.Title className="text-[15px] font-semibold">New workspace</Dialog.Title>
              <Dialog.Description id={ids.desc} className="text-[12.5px] text-shodh-text-muted">
                A workspace keeps the sources, instructions and chats of one piece of work together. Chats in it
                search only its sources.
              </Dialog.Description>
            </div>
            <Dialog.Close
              aria-label="Close"
              className={cn('w-8 h-8 inline-flex items-center justify-center rounded-lg text-shodh-text-muted hover:bg-shodh-raised hover:text-shodh-text', FOCUS_RING)}
            >
              <X className="w-4 h-4" aria-hidden="true" />
            </Dialog.Close>
          </header>
          <form
            className="flex-1 min-h-0 overflow-y-auto scrollbar-thin px-5 py-4 flex flex-col gap-4"
            onSubmit={e => {
              e.preventDefault();
              void create();
            }}
          >
            <fieldset className="flex flex-col gap-2 m-0 p-0 border-0">
              <legend id={ids.templates} className={cn(LABEL, 'mb-2')}>Start from</legend>
              {templates === null && !error ? (
                <p role="status" className="flex items-center gap-2 text-[12.5px] text-shodh-text-muted">
                  <Loader2 className="w-4 h-4 animate-spin motion-reduce:animate-none" aria-hidden="true" /> Loading templates…
                </p>
              ) : (
                <div role="radiogroup" aria-labelledby={ids.templates} className="grid grid-cols-1 sm:grid-cols-2 gap-2">
                  {(templates ?? []).map(t => {
                    const selected = t.id === template;
                    return (
                      <button
                        key={t.id}
                        type="button"
                        role="radio"
                        aria-checked={selected}
                        onClick={() => setTemplate(t.id)}
                        className={cn(
                          'flex items-start gap-2.5 p-3 rounded-xl border text-left transition-colors duration-micro',
                          selected ? 'border-shodh-accent bg-shodh-accent-soft' : 'border-shodh-border hover:bg-shodh-raised',
                          FOCUS_RING,
                        )}
                      >
                        <WorkspaceIcon icon={t.icon} color={t.color} className="w-4 h-4 mt-0.5" />
                        <span className="min-w-0 flex-1">
                          <span className="block text-[13px] font-semibold text-shodh-text">{t.id === 'blank' ? 'Blank' : t.name}</span>
                          <span className="block text-[12px] text-shodh-text-muted">{t.description}</span>
                        </span>
                        {selected && <Check className="w-4 h-4 text-shodh-accent-text shrink-0" aria-hidden="true" />}
                      </button>
                    );
                  })}
                </div>
              )}
            </fieldset>
            {chosen && chosen.instructions && (
              <details className="rounded-lg border border-shodh-border-subtle bg-shodh-raised px-3 py-2">
                <summary className={cn('cursor-pointer text-[12.5px] text-shodh-text-secondary rounded', FOCUS_RING)}>
                  Starting instructions of “{chosen.name}” (you can edit them after creating)
                </summary>
                <p className="mt-2 mb-0 whitespace-pre-wrap text-[12.5px] text-shodh-text-secondary">{chosen.instructions}</p>
              </details>
            )}
            <div className="flex flex-col gap-1">
              <label htmlFor={ids.name} className={LABEL}>Name</label>
              <input
                id={ids.name}
                className={INPUT}
                value={name}
                maxLength={80}
                onChange={e => setName(e.target.value)}
                placeholder={chosen && chosen.id !== 'blank' ? `e.g. ${chosen.name} — spring 2026` : 'e.g. Thesis chapter 2'}
                autoFocus
                required
              />
            </div>
            <div className="flex flex-col gap-1">
              <label htmlFor={ids.description} className={LABEL}>Description (optional)</label>
              <textarea
                id={ids.description}
                className={TEXTAREA}
                rows={2}
                maxLength={500}
                value={description}
                onChange={e => setDescription(e.target.value)}
                placeholder="What this work is about"
              />
            </div>
            {error && (
              <p role="alert" className="m-0 text-[12.5px] text-shodh-error">{error}</p>
            )}
            <div className="flex items-center justify-end gap-2 pt-1">
              <Dialog.Close className={QUIET_BUTTON} type="button">Cancel</Dialog.Close>
              <button type="submit" className={PRIMARY_BUTTON} disabled={!canCreate}>
                {saving && <Loader2 className="w-4 h-4 animate-spin motion-reduce:animate-none" aria-hidden="true" />}
                Create workspace
              </button>
            </div>
          </form>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
