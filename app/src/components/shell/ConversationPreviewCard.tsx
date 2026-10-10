import React, { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { FileText, MessageSquare } from 'lucide-react';
import { conversationPreview } from '../../lib/conversationPreview';
import type { Conversation } from '../../hooks/useConversations';

/** Hover must rest this long before the card shows (avoids flicker while scanning). */
export const PREVIEW_DELAY_MS = 450;
const CARD_WIDTH = 320;
const GAP = 10;
const MARGIN = 8;

function formatDate(iso: string): string {
  const d = new Date(iso);
  return Number.isNaN(d.getTime()) ? '' : d.toLocaleDateString(undefined, { day: 'numeric', month: 'short', year: 'numeric' });
}

/**
 * What a chat is about, beside its sidebar row: opening question, gist of the
 * latest answer, sources used, size and dates. Positioned with `fixed` in a
 * portal so the scrolling sidebar cannot clip it. Purely descriptive: it is
 * `role="tooltip"` and referenced by the row's `aria-describedby`.
 */
export function ConversationPreviewCard({
  id,
  conversation,
  anchor,
}: {
  id: string;
  conversation: Conversation;
  anchor: HTMLElement;
}) {
  const preview = useMemo(() => conversationPreview(conversation.messages), [conversation.messages]);
  const cardRef = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState<{ left: number; top: number } | null>(null);

  useLayoutEffect(() => {
    const place = () => {
      const rect = anchor.getBoundingClientRect();
      const height = cardRef.current?.offsetHeight ?? 160;
      const left = Math.min(rect.right + GAP, window.innerWidth - CARD_WIDTH - MARGIN);
      const top = Math.max(MARGIN, Math.min(rect.top - 6, window.innerHeight - height - MARGIN));
      setPos({ left, top });
    };
    place();
    window.addEventListener('resize', place);
    return () => window.removeEventListener('resize', place);
  }, [anchor, preview]);

  const created = formatDate(conversation.createdAt);
  const updated = formatDate(conversation.updatedAt);
  const empty = !preview.firstQuestion && !preview.latestAnswer;

  return createPortal(
    <div
      id={id}
      ref={cardRef}
      role="tooltip"
      style={{ width: CARD_WIDTH, left: pos?.left ?? -9999, top: pos?.top ?? -9999 }}
      className="shell-pop fixed z-[60] pointer-events-none rounded-xl border border-shodh-border-strong bg-shodh-raised shadow-[0_12px_32px_rgba(0,0,0,0.28)] p-3.5 flex flex-col gap-2.5"
    >
      <p className="text-[13px] font-semibold text-shodh-text leading-snug">{conversation.title}</p>
      {empty ? (
        <p className="text-[12.5px] text-shodh-text-muted">No messages yet.</p>
      ) : (
        <>
          {preview.firstQuestion && (
            <div className="flex flex-col gap-0.5">
              <span className="text-[10.5px] font-semibold uppercase tracking-[0.08em] text-shodh-text-faint">Started with</span>
              <p className="text-[12.5px] leading-snug text-shodh-text-secondary">{preview.firstQuestion}</p>
            </div>
          )}
          {preview.latestAnswer && (
            <div className="flex flex-col gap-0.5">
              <span className="text-[10.5px] font-semibold uppercase tracking-[0.08em] text-shodh-text-faint">Latest answer</span>
              <p className="text-[12.5px] leading-snug text-shodh-text-secondary">{preview.latestAnswer}</p>
            </div>
          )}
          {preview.sources.length > 0 && (
            <div className="flex flex-wrap gap-1">
              {preview.sources.map(s => (
                <span key={s} className="inline-flex items-center gap-1 max-w-full px-1.5 py-0.5 rounded-md bg-shodh-raised-2 text-[11px] text-shodh-text-secondary">
                  <FileText className="w-3 h-3 shrink-0" aria-hidden="true" />
                  <span className="truncate">{s}</span>
                </span>
              ))}
            </div>
          )}
        </>
      )}
      <p className="flex items-center gap-1.5 text-[11px] text-shodh-text-faint">
        <MessageSquare className="w-3 h-3" aria-hidden="true" />
        {preview.questionCount} {preview.questionCount === 1 ? 'question' : 'questions'}
        {created && <> · started {created}</>}
        {updated && updated !== created && <> · updated {updated}</>}
      </p>
    </div>,
    document.body,
  );
}

/**
 * Show/hide state for a row's preview: opens after a hover pause or on
 * keyboard focus, closes on leave, blur, Escape, scroll or click.
 */
export function usePreviewVisibility() {
  const [open, setOpen] = useState(false);
  const timer = useRef<number | null>(null);

  const clear = () => {
    if (timer.current !== null) window.clearTimeout(timer.current);
    timer.current = null;
  };
  const show = () => {
    clear();
    timer.current = window.setTimeout(() => setOpen(true), PREVIEW_DELAY_MS);
  };
  const hide = () => {
    clear();
    setOpen(false);
  };

  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => { if (e.key === 'Escape') hide(); };
    const onScroll = () => hide();
    window.addEventListener('keydown', onKey);
    window.addEventListener('scroll', onScroll, true);
    return () => {
      window.removeEventListener('keydown', onKey);
      window.removeEventListener('scroll', onScroll, true);
    };
  }, [open]);

  useEffect(() => clear, []);

  return { open, show, hide };
}
