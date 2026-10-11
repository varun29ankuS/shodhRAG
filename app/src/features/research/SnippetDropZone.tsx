import React, { useState } from 'react';
import { cn } from '../../lib/utils';
import { readSnippet, snippetChatContext, SNIPPET_DRAG_TYPE } from './snippetModel';
import type { Snippet } from './types';

/** Starts dragging a snippet (to drop it on a composer as context). */
export function startSnippetDrag(event: React.DragEvent, snippet: Snippet): void {
  event.dataTransfer.effectAllowed = 'copy';
  event.dataTransfer.setData(SNIPPET_DRAG_TYPE, JSON.stringify(snippet));
  event.dataTransfer.setData('text/plain', snippetChatContext(snippet));
}

function carriesSnippet(event: React.DragEvent): boolean {
  return Array.from(event.dataTransfer.types).includes(SNIPPET_DRAG_TYPE);
}

/**
 * Wraps a composer: a snippet dropped on it is inserted as a context block
 * (`onInsert`). Other drags (files) pass through to the view's own handlers.
 */
export function SnippetDropZone({ onInsert, children, className }: { onInsert: (text: string) => void; children: React.ReactNode; className?: string }) {
  const [over, setOver] = useState(false);
  return (
    <div
      className={cn('relative', className)}
      onDragEnter={e => {
        if (!carriesSnippet(e)) return;
        e.preventDefault();
        e.stopPropagation();
        setOver(true);
      }}
      onDragOver={e => {
        if (!carriesSnippet(e)) return;
        e.preventDefault();
        e.stopPropagation();
        e.dataTransfer.dropEffect = 'copy';
      }}
      onDragLeave={e => {
        if (!carriesSnippet(e)) return;
        if (e.currentTarget.contains(e.relatedTarget as Node | null)) return;
        setOver(false);
      }}
      onDrop={e => {
        if (!carriesSnippet(e)) return;
        e.preventDefault();
        e.stopPropagation();
        setOver(false);
        let parsed: unknown = null;
        try {
          parsed = JSON.parse(e.dataTransfer.getData(SNIPPET_DRAG_TYPE));
        } catch {
          parsed = null;
        }
        const snippet = readSnippet(parsed);
        if (snippet) onInsert(snippetChatContext(snippet));
      }}
    >
      {children}
      {over && (
        <div
          className="pointer-events-none absolute inset-0 z-10 flex items-center justify-center rounded-[14px] border-2 border-dashed border-shodh-accent bg-shodh-ground/70 text-[13px] font-medium text-shodh-text"
          aria-hidden="true"
        >
          Drop to add the snippet as context
        </div>
      )}
    </div>
  );
}
