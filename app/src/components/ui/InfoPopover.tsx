import React, { useEffect, useId } from 'react';
import { createPortal } from 'react-dom';
import { Info, X } from 'lucide-react';
import { cn } from '../../lib/utils';
import { usePopover } from '../../hooks/usePopover';
import { IS_MAC_PLATFORM, withShortcut } from '../../lib/shortcuts';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1 focus-visible:ring-offset-shodh-surface';

/**
 * A small "i" button that opens an explanation in a pop-up, in place of a
 * paragraph that would sit in the page. The pop-up takes focus when it opens;
 * Esc, Tab, the close button or a press outside close it and focus goes back to
 * the button. It is a dialog, so the views' own Esc handling leaves the key to it
 * (lib/escapeScope).
 */
export function InfoPopover({
  label,
  title,
  children,
  className,
}: {
  /** What the explanation is about, for the button's name: "About backup models". */
  label: string;
  /** The pop-up's heading. */
  title: string;
  children: React.ReactNode;
  className?: string;
}) {
  const { open, setOpen, close, pos, triggerRef, popRef } = usePopover();
  const titleId = useId();
  const dialogId = useId();

  useEffect(() => {
    if (open) popRef.current?.focus();
  }, [open, popRef]);

  return (
    <>
      <button
        ref={triggerRef}
        type="button"
        aria-label={label}
        title={label}
        aria-haspopup="dialog"
        aria-expanded={open}
        aria-controls={open ? dialogId : undefined}
        onClick={e => {
          e.stopPropagation();
          setOpen(!open);
        }}
        className={cn(
          'shrink-0 w-6 h-6 inline-flex items-center justify-center rounded-md border border-transparent text-shodh-text-muted hover:text-shodh-text hover:bg-shodh-raised transition-colors duration-micro',
          open && 'text-shodh-text bg-shodh-raised',
          FOCUS_RING,
          className,
        )}
      >
        <Info className="w-3.5 h-3.5" aria-hidden="true" />
      </button>
      {open &&
        createPortal(
          <div
            ref={popRef}
            id={dialogId}
            role="dialog"
            aria-labelledby={titleId}
            tabIndex={-1}
            data-esc-local=""
            onClick={e => e.stopPropagation()}
            onKeyDown={e => {
              if (e.key === 'Escape') {
                e.preventDefault();
                e.stopPropagation();
                close(true);
              } else if (e.key === 'Tab') {
                // Tab reaches the close button; past it (or back from the pop-up) it closes,
                // since the pop-up lives in a body-level portal outside the page order.
                const onPopup = document.activeElement === popRef.current;
                if (onPopup && !e.shiftKey) return;
                e.preventDefault();
                close(true);
              }
            }}
            className="fixed z-50 w-[min(360px,calc(100vw-16px))] rounded-xl border border-shodh-border-strong bg-shodh-surface p-3 shadow-[0_12px_36px_rgba(0,0,0,0.32)] shell-pop focus:outline-none"
            style={pos ? { left: pos.left, top: pos.top } : { opacity: 0, left: 0, top: 0 }}
          >
            <div className="flex items-start gap-2">
              <h4 id={titleId} className="m-0 flex-1 min-w-0 text-small font-semibold text-shodh-text">
                {title}
              </h4>
              <button
                type="button"
                onClick={() => close(true)}
                aria-label="Close"
                title={withShortcut('Close', 'close', IS_MAC_PLATFORM)}
                className={cn(
                  '-mt-1 -mr-1 w-6 h-6 inline-flex items-center justify-center rounded-md text-shodh-text-muted hover:text-shodh-text hover:bg-shodh-raised',
                  FOCUS_RING,
                )}
              >
                <X className="w-3.5 h-3.5" aria-hidden="true" />
              </button>
            </div>
            <div className="mt-1.5 flex flex-col gap-2 text-small leading-[1.55] text-shodh-text-secondary">{children}</div>
          </div>,
          document.body,
        )}
    </>
  );
}
