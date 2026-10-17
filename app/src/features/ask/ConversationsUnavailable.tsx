import { useEffect, useRef } from 'react';
import type { KeyboardEvent } from 'react';
import { useTheme } from '../../contexts/ThemeContext';
import type { LoadFailureNotice } from '../../hooks/conversationSaves';

/**
 * Blocks the window when the saved conversations could not be loaded: says
 * what happened, that saving is paused so the file is not overwritten, and
 * where the file is. The user acknowledges it to keep working; saving stays
 * paused for the session.
 */
export function ConversationsUnavailable({ notice, onContinue }: {
  notice: LoadFailureNotice;
  onContinue: () => void;
}) {
  const { colors } = useTheme();
  const button = useRef<HTMLButtonElement>(null);
  const dialog = useRef<HTMLDivElement>(null);
  useEffect(() => {
    button.current?.focus();
    // Focus that leaves the dialog (a click behind it) comes back to it.
    const onFocusIn = (event: FocusEvent) => {
      const target = event.target;
      if (dialog.current && target instanceof Node && !dialog.current.contains(target)) {
        button.current?.focus();
      }
    };
    document.addEventListener('focusin', onFocusIn);
    return () => document.removeEventListener('focusin', onFocusIn);
  }, []);
  // Tab and Shift+Tab stay inside the dialog: the window behind it is blocked.
  const keepFocusInside = (event: KeyboardEvent<HTMLDivElement>) => {
    if (event.key !== 'Tab' || !dialog.current) return;
    const focusable = Array.from(
      dialog.current.querySelectorAll<HTMLElement>(
        'button:not([disabled]), [href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])',
      ),
    );
    if (focusable.length === 0) {
      event.preventDefault();
      return;
    }
    const first = focusable[0];
    const last = focusable[focusable.length - 1];
    const active = document.activeElement;
    const inside = active instanceof HTMLElement && dialog.current.contains(active);
    if (event.shiftKey && (active === first || !inside)) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && (active === last || !inside)) {
      event.preventDefault();
      first.focus();
    }
  };
  return (
    <div
      style={{
        position: 'fixed',
        inset: 0,
        zIndex: 10000,
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'center',
        padding: 16,
        background: 'rgba(0, 0, 0, 0.45)',
      }}
    >
      <div
        ref={dialog}
        onKeyDown={keepFocusInside}
        role="alertdialog"
        aria-modal="true"
        aria-labelledby="conversations-unavailable-title"
        aria-describedby="conversations-unavailable-body"
        style={{
          maxWidth: 520,
          width: '100%',
          borderRadius: 12,
          padding: '20px 22px',
          background: colors.cardBg,
          color: colors.text,
          border: `1px solid ${colors.border}`,
          boxShadow: '0 12px 32px rgba(0, 0, 0, 0.3)',
        }}
      >
        <h2
          id="conversations-unavailable-title"
          style={{ margin: '0 0 12px', fontSize: 16, fontWeight: 600, color: colors.text }}
        >
          {notice.title}
        </h2>
        <div id="conversations-unavailable-body">
          {notice.paragraphs.map(paragraph => (
            <p
              key={paragraph}
              style={{
                margin: '0 0 10px',
                fontSize: 13,
                lineHeight: 1.5,
                color: colors.textSecondary,
                overflowWrap: 'anywhere',
              }}
            >
              {paragraph}
            </p>
          ))}
        </div>
        <div style={{ display: 'flex', justifyContent: 'flex-end', marginTop: 8 }}>
          <button
            ref={button}
            type="button"
            onClick={onContinue}
            style={{
              padding: '8px 14px',
              borderRadius: 8,
              border: 'none',
              cursor: 'pointer',
              fontSize: 13,
              fontWeight: 500,
              background: colors.primary,
              color: colors.primaryText,
            }}
          >
            Continue without saving
          </button>
        </div>
      </div>
    </div>
  );
}
