import React, { useEffect, useId, useState } from 'react';
import * as Dialog from '@radix-ui/react-dialog';
import { AlertTriangle, Loader2, Play, X } from 'lucide-react';
import { cn } from '../../lib/utils';
import { notify } from '../../lib/notify';
import { tableError, tablesApi } from './api';
import { callsLine, capNotice, estimateText, fillStartOutcome, reasonText } from './format';
import type { EstimateView, TableFailure } from './types';
import {
  CLOSE_BUTTON,
  DIALOG_BODY,
  DIALOG_CONTENT,
  DIALOG_HEADER,
  DIALOG_OVERLAY,
  FOCUS_RING,
  openSettings,
  OUTLINE_BUTTON,
  PRIMARY_BUTTON,
  QUIET_BUTTON,
} from './ui';

type Load = { status: 'loading' } | { status: 'ready'; estimate: EstimateView } | { status: 'error'; failure: TableFailure };

/** The model codes that mean "choose or fix the model", answered with a way to Settings → Model. */
export const MODEL_CODES: readonly string[] = ['model_refused', 'model_unavailable'];

/** A model the fill cannot use, said plainly with what to do. */
export function ModelProblem({ failure, onOpenSettings }: { failure: TableFailure; onOpenSettings?: () => void }) {
  return (
    <div role="alert" className="flex items-start gap-2.5 rounded-lg border border-shodh-warning/40 bg-shodh-warning/10 px-3 py-2.5">
      <AlertTriangle className="w-4 h-4 mt-0.5 shrink-0 text-shodh-warning" aria-hidden="true" />
      <div className="min-w-0 flex flex-col gap-1.5">
        <p className="m-0 text-small font-semibold text-shodh-text">
          {failure.code === 'model_refused' ? 'This model cannot fill tables' : 'No model can fill tables now'}
        </p>
        <p className="m-0 text-small text-shodh-text-secondary">{failure.message || reasonText(failure.code)}</p>
        <button
          type="button"
          className={cn(OUTLINE_BUTTON, 'self-start')}
          onClick={() => {
            onOpenSettings?.();
            openSettings('models');
          }}
        >
          Open Settings → Model
        </button>
      </div>
    </div>
  );
}

/**
 * Fill: what filling the table needs (papers × columns, the model calls, the cells the
 * cache answers), the day's calls with the local reset, and what the cap means for this
 * fill. A model tables cannot use is said before any fill starts. Start says honestly what
 * happened (`fill_start`).
 */
export function FillDialog({
  open,
  onOpenChange,
  tableId,
  onStarted,
  onModelProblem,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  tableId: string;
  onStarted: () => void;
  /** A refused or missing model, so the table can keep saying so after the dialog closes. */
  onModelProblem: (failure: TableFailure | null) => void;
}) {
  const descriptionId = useId();
  const [load, setLoad] = useState<Load>({ status: 'loading' });
  const [starting, setStarting] = useState(false);
  const [startError, setStartError] = useState<TableFailure | null>(null);

  useEffect(() => {
    if (!open) return undefined;
    let cancelled = false;
    setLoad({ status: 'loading' });
    setStartError(null);
    tablesApi
      .estimate(tableId)
      .then(estimate => {
        if (cancelled) return;
        setLoad({ status: 'ready', estimate });
        onModelProblem(null);
      })
      .catch(err => {
        if (cancelled) return;
        const failure = tableError(err);
        setLoad({ status: 'error', failure });
        if (MODEL_CODES.includes(failure.code)) onModelProblem(failure);
      });
    return () => { cancelled = true; };
  }, [open, tableId, onModelProblem]);

  const start = async (estimate: EstimateView) => {
    setStarting(true);
    setStartError(null);
    try {
      const result = await tablesApi.start(tableId, estimate.calls);
      const outcome = fillStartOutcome(result, Date.now());
      if (outcome.tone === 'ok') notify.success(outcome.text);
      else if (outcome.tone === 'warning') notify.warning(outcome.text);
      else notify.info(outcome.text);
      onStarted();
      onOpenChange(false);
    } catch (err) {
      const failure = tableError(err);
      setStartError(failure);
      if (MODEL_CODES.includes(failure.code)) onModelProblem(failure);
    } finally {
      setStarting(false);
    }
  };

  const now = Date.now();

  return (
    <Dialog.Root open={open} onOpenChange={onOpenChange}>
      <Dialog.Portal>
        <Dialog.Overlay className={DIALOG_OVERLAY} />
        <Dialog.Content aria-describedby={descriptionId} className={DIALOG_CONTENT}>
          <header className={DIALOG_HEADER}>
            <div className="flex-1 min-w-0">
              <Dialog.Title className="text-body-lg font-semibold">Fill the table</Dialog.Title>
              <Dialog.Description id={descriptionId} className="text-small text-shodh-text-muted">
                The model reads each paper's best passages for your columns and answers with a quote, which Shodh
                checks word for word against the paper. Cells you edited are never overwritten.
              </Dialog.Description>
            </div>
            <Dialog.Close aria-label="Close" className={cn(CLOSE_BUTTON, FOCUS_RING)}>
              <X className="w-4 h-4" aria-hidden="true" />
            </Dialog.Close>
          </header>
          <div className={DIALOG_BODY}>
            {load.status === 'loading' && (
              <p role="status" className="m-0 flex items-center gap-2 text-small text-shodh-text-muted">
                <Loader2 className="w-4 h-4 animate-spin motion-reduce:animate-none" aria-hidden="true" /> Working out what the fill needs…
              </p>
            )}
            {load.status === 'error' && (MODEL_CODES.includes(load.failure.code) ? (
              <ModelProblem failure={load.failure} onOpenSettings={() => onOpenChange(false)} />
            ) : (
              <p role="alert" className="m-0 text-small text-shodh-error">
                What the fill needs could not be worked out: {load.failure.message || reasonText(load.failure.code)}
              </p>
            ))}
            {load.status === 'ready' && (
              <>
                {load.estimate.cells === 0 ? (
                  <p className="m-0 text-small text-shodh-text">
                    Every cell is filled, edited or answered already; there is nothing to ask the model.
                  </p>
                ) : (
                  <div className="flex flex-col gap-1.5">
                    <p className="m-0 text-body text-shodh-text">
                      {estimateText(load.estimate.papers, load.estimate.columns, load.estimate.calls)}
                    </p>
                    {load.estimate.cached > 0 && (
                      <p className="m-0 text-small text-shodh-text-secondary">
                        {load.estimate.cached} {load.estimate.cached === 1 ? 'cell is' : 'cells are'} answered from earlier fills of
                        the same paper and question, without a call.
                      </p>
                    )}
                    <p className="m-0 text-small text-shodh-text-muted">
                      A paper whose reply cannot be read is asked once more, which counts as a call too.
                    </p>
                  </div>
                )}
                <p className="m-0 text-small text-shodh-text-secondary">
                  {callsLine(load.estimate.callsToday, load.estimate.cap, load.estimate.dayResetsAt, now)}
                </p>
                {load.estimate.cells > 0 && (() => {
                  const notice = capNotice(load.estimate.calls, load.estimate.callsToday, load.estimate.cap, load.estimate.dayResetsAt, now);
                  return notice ? (
                    <p className="m-0 text-small text-shodh-warning">
                      {notice}{' '}
                      <button
                        type="button"
                        className={cn('underline underline-offset-2 rounded', FOCUS_RING)}
                        onClick={() => {
                          onOpenChange(false);
                          openSettings('tables');
                        }}
                      >
                        Settings → Tables
                      </button>
                    </p>
                  ) : null;
                })()}
              </>
            )}
            {startError && (MODEL_CODES.includes(startError.code) ? (
              <ModelProblem failure={startError} onOpenSettings={() => onOpenChange(false)} />
            ) : (
              <p role="alert" className="m-0 text-small text-shodh-error">
                The fill did not start: {startError.message || reasonText(startError.code)}
              </p>
            ))}
            <div className="flex items-center justify-end gap-2 pt-1">
              <Dialog.Close className={QUIET_BUTTON} type="button">Close</Dialog.Close>
              <button
                type="button"
                className={PRIMARY_BUTTON}
                disabled={load.status !== 'ready' || load.estimate.cells === 0 || starting}
                onClick={() => { if (load.status === 'ready') void start(load.estimate); }}
              >
                {starting ? <Loader2 className="w-4 h-4 animate-spin motion-reduce:animate-none" aria-hidden="true" /> : <Play className="w-4 h-4" aria-hidden="true" />}
                Start filling
              </button>
            </div>
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
