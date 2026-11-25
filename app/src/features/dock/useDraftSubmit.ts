/**
 * Sending the shared draft from a chat outside Ask, the in-page chat or the floating chat over
 * the embedded preview: one path for both (`submitDockDraft`), with Ask's rules. The attached
 * image text goes with the typed words, a chosen file limits the search and is checked first,
 * and a question asked before Shodh is ready waits for setup.
 */
import { useCallback, useRef } from 'react';
import { useChatSession } from '../ask/ChatSessionContext';
import { composerDrafts } from '../ask/draftStore';
import { useSendOptions } from '../ask/useSendOptions';
import { useChatFileScope, useImageAttachments } from '../ask/AttachmentChips';
import type { ConversationFile, FileRef } from '../ask/fileQuestion';
import { attachedContextOf, composerAttachments } from '../ask/imageAttachments';
import { composerFiles, limitedToFiles } from '../ask/fileQuestion';
import { missingIndexedFiles, reportMissingFiles } from '../ask/fileQuestionApi';
import type { OptionSource } from '../ask/sendOptions';
import { useQueuedQuestion } from '../setup/useQueuedQuestion';
import { submitDockDraft } from './dockActions';

export function useDraftSubmit(sources: readonly OptionSource[]) {
  const session = useChatSession();
  const { isStreaming, send, streamingConversationId, sideRun } = session;
  const conversationId = session.activeConversationId;
  const sendOptions = useSendOptions(sources);
  // Image text attached in Ask goes out with a reply sent from here too.
  const attachments = useImageAttachments(conversationId);
  // As in Ask: an "About <file>" chat stays limited to that file until its chip is removed.
  const saveAboutFile = useCallback((file: ConversationFile | null) => {
    const id = session.activeConversationId;
    if (id) session.updateConversationMeta(id, { aboutFile: file ?? undefined });
  }, [session.activeConversationId, session.updateConversationMeta]);
  const fileScope = useChatFileScope(conversationId, session.activeConversation?.aboutFile, saveAboutFile);
  // Read after the file check: a conversation switch meanwhile cancels the send.
  const shownConversationRef = useRef(conversationId);
  shownConversationRef.current = conversationId;
  const checkingFileRef = useRef(false);
  // As in Ask: a question asked before Shodh is ready waits for setup.
  const queued = useQueuedQuestion(conversationId);
  const busyElsewhere = (streamingConversationId !== null && !isStreaming) || sideRun !== null;

  const submit = (draft: string) => {
    let attached = composerAttachments.get(conversationId);
    let file: FileRef | null = null;
    submitDockDraft(draft, {
      conversationId,
      isStreaming,
      busyElsewhere,
      checking: () => checkingFileRef.current,
      setChecking: on => {
        checkingFileRef.current = on;
      },
      currentDraft: () => composerDrafts.get(conversationId),
      queuedFile: () => {
        file = composerFiles.get(conversationId);
        return file;
      },
      send: (text, files) => {
        attached = composerAttachments.get(conversationId);
        return send(text, limitedToFiles(sendOptions, files), { attachedContext: attachedContextOf(attached) });
      },
      sent: () => {
        composerDrafts.set(conversationId, '');
        composerAttachments.consume(conversationId, attached);
        if (file) fileScope.keep(file);
      },
      hold: sendNow => queued.hold(sendNow),
      missing: files => missingIndexedFiles([...files]),
      reportMissing: missing => reportMissingFiles([...missing], false, fileScope.remove),
      shownConversation: () => shownConversationRef.current,
    });
  };

  return { submit, attachments, fileScope, queued, busyElsewhere };
}
