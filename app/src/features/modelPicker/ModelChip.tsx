import { useCallback, useEffect, useId, useLayoutEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { ChevronDown } from 'lucide-react';
import { cn } from '../../lib/utils';
import { notify } from '../../lib/notify';
import { OPEN_MODEL_PICKER_EVENT, useModelPicker } from './modelApi';
import { ChipMenu } from './ChipMenu';
import { modelName, sourceText } from './modelFormat';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1 focus-visible:ring-offset-shodh-surface';

const PANEL_WIDTH = 340;
const EDGE = 8;

interface ModelChipProps {
  /** An answer is running in this conversation (a choice applies to the next one). */
  answerRunning: boolean;
  /** Open Settings → Model. */
  onOpenSettings: () => void;
}

/**
 * The model chip in the Ask composer: the model the next answer uses (and
 * whether the environment or this session set it). Opens a short menu above
 * the composer with the connected picks and recently used models, and
 * "Manage…" (Settings → Model); a choice applies from the next answer,
 * without a restart, and never interrupts the answer in progress.
 */
export function ModelChip({ answerRunning, onOpenSettings }: ModelChipProps) {
  const picker = useModelPicker();
  const { view } = picker;
  const [open, setOpen] = useState(false);
  const [pos, setPos] = useState<{ left: number; bottom: number } | null>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const panelRef = useRef<HTMLDivElement>(null);
  const panelId = useId();

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
    const onOpen = () => setOpen(true);
    window.addEventListener(OPEN_MODEL_PICKER_EVENT, onOpen);
    return () => window.removeEventListener(OPEN_MODEL_PICKER_EVENT, onOpen);
  }, []);

  // Reopening shows current prices and installed local models.
  useEffect(() => {
    if (open) void picker.reload(false);
    // `picker.reload` is stable (useCallback with no dependencies).
  }, [open, picker.reload]);

  const active = view?.active ?? null;
  const label = active ? modelName(view?.models ?? [], active.model) : view ? 'Choose a model' : 'Model';
  const source = sourceText(active);

  return (
    <>
      <button
        ref={triggerRef}
        type="button"
        onClick={() => setOpen(o => !o)}
        aria-haspopup="dialog"
        aria-expanded={open}
        aria-controls={open ? panelId : undefined}
        aria-label={`Model: ${label}${source ? ` (${source.toLowerCase()})` : ''}. Change model`}
        title={source ? `${label} (${source})` : label}
        className={cn(
          'inline-flex items-center gap-[7px] h-[30px] px-2.5 rounded-full bg-shodh-raised-2 text-[12.5px] text-shodh-text-secondary hover:bg-shodh-pressed hover:text-shodh-text transition-colors duration-micro min-w-0',
          FOCUS_RING,
        )}
      >
        <span className={cn('w-[7px] h-[7px] rounded-full shrink-0', active ? 'bg-shodh-info' : 'bg-shodh-warning')} aria-hidden="true" />
        <span className="truncate max-w-[200px]">{label}</span>
        {source && <span className="shrink-0 text-[11px] text-shodh-text-faint">· {source}</span>}
        <ChevronDown className="w-3 h-3 shrink-0" aria-hidden="true" />
      </button>
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
