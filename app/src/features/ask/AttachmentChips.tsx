import React, { useCallback, useEffect, useSyncExternalStore } from 'react';
import { FileText, X } from 'lucide-react';
import { cn } from '../../lib/utils';
import { attachmentLabel, composerAttachments } from './imageAttachments';
import type { ImageTextAttachment } from './imageAttachments';
import { composerFiles, keptFile, readConversationFile, sameConversationFile } from './fileQuestion';
import type { ConversationFile, FileRef } from './fileQuestion';

/** The attachments waiting for the next question of `conversationId` (see `imageAttachments`). */
export function useImageAttachments(conversationId: string | null): readonly ImageTextAttachment[] {
  const subscribe = useCallback((listener: () => void) => composerAttachments.subscribe(listener), []);
  return useSyncExternalStore(subscribe, () => composerAttachments.get(conversationId));
}

/** The file the next question of `conversationId` is about (see `fileQuestion`). */
export function useComposerFile(conversationId: string | null): FileRef | null {
  const subscribe = useCallback((listener: () => void) => composerFiles.subscribe(listener), []);
  return useSyncExternalStore(subscribe, () => composerFiles.get(conversationId));
}

/** A chat's file limit: the chip's file, what keeps it after a send, and what removes it. */
export interface ChatFileScope {
  file: FileRef | null;
  /** A question about `sent` went out: the chat stays about it. */
  keep: (sent: FileRef) => void;
  /** The user removed the chip: answers search the whole library again. */
  remove: () => void;
}

/**
 * The file an "About <file>" chat is limited to. It is the composer's file (chosen with
 * "Ask about this file", a paper or a figure), and once a question about it is sent it
 * stays for the rest of the chat: the chip remains, follow-ups search only that file, and
 * the conversation saves it (`saved`, through `save`) so it is still there after a restart.
 */
export function useChatFileScope(
  conversationId: string | null,
  saved: unknown,
  save: (file: ConversationFile | null) => void,
): ChatFileScope {
  const file = useComposerFile(conversationId);
  // A conversation opened with a saved limit shows it again.
  useEffect(() => {
    const stored = readConversationFile(saved);
    if (conversationId && stored && !composerFiles.get(conversationId)) composerFiles.set(conversationId, stored);
  }, [conversationId, saved]);
  const keep = useCallback((sent: FileRef) => {
    const next = keptFile(sent);
    // A file chosen while this question was being sent stays the composer's.
    if (composerFiles.get(conversationId) === sent) composerFiles.set(conversationId, next);
    if (!sameConversationFile(saved, next)) save(next);
  }, [conversationId, saved, save]);
  const remove = useCallback(() => {
    composerFiles.set(conversationId, null);
    if (!sameConversationFile(saved, null)) save(null);
  }, [conversationId, saved, save]);
  return { file, keep, remove };
}

const FOCUS_RING = 'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring';

/**
 * The file the next question is about: named by its title (never its path), and
 * removable, which lets the answer search the whole library again.
 */
export function FileRefChip({
  conversationId,
  file,
  compact = false,
  onRemove,
}: {
  conversationId: string | null;
  file: FileRef;
  compact?: boolean;
  /** Removes the chat's limit too (`useChatFileScope`); without it only the composer's file goes. */
  onRemove?: () => void;
}) {
  return (
    <div className={cn('flex', compact ? 'px-3 pt-2' : 'px-[18px] pt-3')}>
      <span
        className="inline-flex items-center gap-1.5 h-7 max-w-full pl-2.5 pr-1 rounded-full bg-shodh-raised-2 text-[12px] text-shodh-text-secondary"
        title={`Answers in this chat search only ${file.name}, until you remove it`}
      >
        <FileText className="w-3.5 h-3.5 shrink-0 text-shodh-accent-text" aria-hidden="true" />
        <span className="truncate">{`About ${file.name}`}</span>
        <button
          type="button"
          onClick={() => (onRemove ? onRemove() : composerFiles.set(conversationId, null))}
          aria-label={`Search all sources instead of only ${file.name}`}
          title="Search all sources instead"
          className={cn('w-5 h-5 shrink-0 inline-flex items-center justify-center rounded-full text-shodh-text-muted hover:bg-shodh-pressed hover:text-shodh-text', FOCUS_RING)}
        >
          <X className="w-3 h-3" aria-hidden="true" />
        </button>
      </span>
    </div>
  );
}

/** The chips above the composer: one per attachment, each removable. */
export function AttachmentChips({
  conversationId,
  attachments,
  compact = false,
}: {
  conversationId: string | null;
  attachments: readonly ImageTextAttachment[];
  compact?: boolean;
}) {
  if (attachments.length === 0) return null;
  return (
    <ul
      aria-label="Attached to your next question"
      className={cn('flex flex-wrap gap-1.5 m-0 list-none', compact ? 'px-3 pt-2' : 'px-[18px] pt-3')}
    >
      {attachments.map(attachment => {
        const label = attachmentLabel(attachment);
        return (
          <li
            key={attachment.id}
            className="inline-flex items-center gap-1.5 h-7 max-w-full pl-2.5 pr-1 rounded-full bg-shodh-raised-2 text-[12px] text-shodh-text-secondary"
            title={attachment.text.slice(0, 400)}
          >
            <FileText className="w-3.5 h-3.5 shrink-0 text-shodh-accent-text" aria-hidden="true" />
            <span className="truncate">{label}</span>
            <button
              type="button"
              onClick={() => composerAttachments.remove(conversationId, attachment.id)}
              aria-label={`Remove: ${label}`}
              title="Remove"
              className={cn('w-5 h-5 shrink-0 inline-flex items-center justify-center rounded-full text-shodh-text-muted hover:bg-shodh-pressed hover:text-shodh-text', FOCUS_RING)}
            >
              <X className="w-3 h-3" aria-hidden="true" />
            </button>
          </li>
        );
      })}
    </ul>
  );
}
