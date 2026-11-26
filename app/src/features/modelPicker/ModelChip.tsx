import { useCallback, useEffect, useId, useLayoutEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { notify } from '../../lib/notify';
import { ModelTrigger } from './ModelTrigger';
import { OPEN_MODEL_PICKER_EVENT, useModelPicker } from './modelApi';
import { ChipMenu } from './ChipMenu';
import { modelLabelOf, sourceText } from './modelFormat';
import { OPEN_PICKER_REQUEST_EVENT, takeModelPickerRequest } from './navigation';
import { modelDot } from '../connection/connectionStatus';
import { useConnectionSnapshot } from '../connection/useConnectionSnapshot';

const PANEL_WIDTH = 340;
const EDGE = 8;

interface ModelChipProps {
  /** An answer is running in this conversation (a choice applies to the next one). */
  answerRunning: boolean;
  /** Open Settings → Model. */
  onOpenSettings: () => void;
  /**
   * Opens for "Switch model" (`requestModelPicker`, which shows Ask). Only Ask's chip
   * does: the floating chat's chip unmounts as Ask opens, and would take the request away.
   */
  takesPickerRequests?: boolean;
  /** Why the model cannot change now (an answer is running), or null. */
  lockedReason?: string | null;
}

/**
 * The model chip in the Ask composer: the model the next answer uses (and
 * whether the environment or this session set it). Opens a short menu above
 * the composer with the connected picks and recently used models, and
 * "Manage…" (Settings → Model); a choice applies from the next answer,
 * without a restart, and never interrupts the answer in progress.
 */
export function ModelChip({ answerRunning, onOpenSettings, takesPickerRequests = true, lockedReason = null }: ModelChipProps) {
  const picker = useModelPicker();
  const { view } = picker;
  const [open, setOpen] = useState(false);
  const [pos, setPos] = useState<{ left: number; bottom: number } | null>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const panelRef = useRef<HTMLDivElement>(null);
  const panelId = useId();
  // Locked while an answer runs: the menu closes, and requests to open it wait for the end.
  const lockedRef = useRef(lockedReason !== null);
  lockedRef.current = lockedReason !== null;
  useEffect(() => {
    if (lockedReason !== null) setOpen(false);
    else if (takesPickerRequests && takeModelPickerRequest()) setOpen(true);
  }, [lockedReason, takesPickerRequests]);

  const close = useCallback((refocus: boolean) => {
    setOpen(false);
    if (refocus) triggerRef.current?.focus();
  }, []);

  useLayoutEffect(() => {
    if (!open || !triggerRef.current) return;
    const place = () => {
      const anchor = triggerRef.current?.getBoundingClientRect();
      if (!anchor) return;
      const width = Math.min(PANEL_WIDTH, window.innerWidth - EDGE * 2);
      const left = Math.min(Math.max(EDGE, anchor.left), window.innerWidth - width - EDGE);
      setPos({ left, bottom: Math.max(EDGE, window.innerHeight - anchor.top + 6) });
    };
    place();
    window.addEventListener('resize', place);
    return () => window.removeEventListener('resize', place);
  }, [open]);

  useEffect(() => {
    if (!open) return;
    const onPointer = (e: PointerEvent) => {
      const target = e.target as Node;
      if (panelRef.current?.contains(target) || triggerRef.current?.contains(target)) return;
      close(false);
    };
    window.addEventListener('pointerdown', onPointer, true);
    return () => window.removeEventListener('pointerdown', onPointer, true);
  }, [open, close]);

  // "Change model" on an answer's error card opens this picker.
  useEffect(() => {
    const onOpen = () => { if (!lockedRef.current) setOpen(true); };
    window.addEventListener(OPEN_MODEL_PICKER_EVENT, onOpen);
    return () => window.removeEventListener(OPEN_MODEL_PICKER_EVENT, onOpen);
  }, []);

  // "Switch model" in the top bar's connection status: asked for before or
  // after this chip mounted.
  useEffect(() => {
    if (!takesPickerRequests) return;
    if (!lockedRef.current && takeModelPickerRequest()) setOpen(true);
    const onRequest = () => {
      if (!lockedRef.current && takeModelPickerRequest()) setOpen(true);
    };
    window.addEventListener(OPEN_PICKER_REQUEST_EVENT, onRequest);
    return () => window.removeEventListener(OPEN_PICKER_REQUEST_EVENT, onRequest);
  }, [takesPickerRequests]);

  // Reopening shows current prices and installed local models.
  useEffect(() => {
    if (open) void picker.reload(false);
    // `picker.reload` is stable (useCallback with no dependencies).
  }, [open, picker.reload]);

  const active = view?.active ?? null;
  const { label, ready } = modelLabelOf(view);
  // The same state, in the same colour, as the sidebar's model status.
  const { snapshot: connection } = useConnectionSnapshot();
  const source = sourceText(active);

  return (
    <>
      <ModelTrigger
        ref={triggerRef}
        label={label}
        ready={ready}
        dot={modelDot(connection)}
        lockedReason={lockedReason}
        onClick={() => setOpen(o => !o)}
        aria-haspopup="dialog"
        aria-expanded={open}
        aria-controls={open ? panelId : undefined}
        aria-label={`Model: ${label}${source ? ` (${source.toLowerCase()})` : ''}. Change model`}
        title={source ? `${label} · ${source}` : label}
      />
      {open &&
        createPortal(
          <div
            ref={panelRef}
            id={panelId}
            role="dialog"
            aria-label="Choose the model"
            onKeyDown={e => {
              if (e.key === 'Escape') {
                e.preventDefault();
                e.stopPropagation();
                close(true);
              }
            }}
            className="shell-pop fixed z-50 flex flex-col rounded-xl border border-shodh-border-strong bg-shodh-surface shadow-[0_12px_36px_rgba(0,0,0,0.32)] overflow-hidden"
            style={{
              left: pos?.left ?? EDGE,
              bottom: pos?.bottom ?? EDGE,
              width: `min(${PANEL_WIDTH}px, calc(100vw - ${EDGE * 2}px))`,
              maxHeight: `calc(100vh - ${EDGE * 2}px)`,
              visibility: pos ? 'visible' : 'hidden',
            }}
          >
            <ChipMenu
              picker={picker}
              answerRunning={answerRunning}
              onManage={() => {
                close(false);
                onOpenSettings();
              }}
              onSelected={(name, changed) => {
                close(true);
                if (changed) {
                  notify.success(answerRunning ? `${name} will answer your next question` : `${name} will answer from now on`);
                }
              }}
            />
          </div>,
          document.body,
        )}
    </>
  );
}
