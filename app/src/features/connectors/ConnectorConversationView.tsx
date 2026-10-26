import React from 'react';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import { openUrl } from '@tauri-apps/plugin-opener';
import { ExternalLink, MessageSquarePlus, Lock } from 'lucide-react';
import { cn } from '../../lib/utils';
import { notify } from '../../lib/notify';
import type { Conversation } from '../../hooks/useConversations';
import { connectorName, continueSeed, messageAuthor, readOnlyLine } from './conversations';
import { ConnectorIcon } from './ConnectorIcon';
import { FOCUS_RING, OUTLINE_BUTTON, PRIMARY_BUTTON } from './ui';

function time(at: string): string {
  const date = new Date(at);
  if (Number.isNaN(date.getTime())) return '';
  return date.toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' });
}

/** Links in answers open in the browser, never inside the app. */
const MARKDOWN_COMPONENTS = {
  a: ({ href, children }: { href?: string; children?: React.ReactNode }) => (
    <a
      href={href}
      onClick={e => {
        e.preventDefault();
        if (href && /^https:\/\//.test(href)) void openUrl(href).catch(() => notify.error('The link could not be opened'));
      }}
      className="text-shodh-accent-text underline underline-offset-2"
    >
      {children}
    </a>
  ),
};

/**
 * A conversation that happened in Slack or Telegram, read-only: who asked (the user, or
 * someone else by name), Shodh's answers, and "Continue in Ask", which starts an Ask chat
 * carrying it as quoted context.
 */
export function ConnectorConversationView({
  conversation,
  onContinue,
}: {
  conversation: Conversation;
  onContinue: (conversation: Conversation) => void;
}) {
  const source = conversation.source;
  if (!source) return null;
  const name = connectorName(source.connector);
  const canContinue = continueSeed(conversation) !== null;
  const link = source.link && /^https:\/\//.test(source.link) ? source.link : null;

  return (
    <section aria-label={`${conversation.title}, from ${name}`} className="relative h-full flex flex-col bg-shodh-ground">
      <header className="shrink-0 border-b border-shodh-border bg-shodh-surface/70 px-7 py-4">
        <div className="max-w-[700px] mx-auto flex flex-wrap items-center gap-x-4 gap-y-3">
          <div className="min-w-0 flex-1 flex items-center gap-3">
            <span className="w-9 h-9 shrink-0 rounded-[10px] bg-white border border-black/10 dark:border-white/10 inline-flex items-center justify-center" aria-hidden="true">
              <ConnectorIcon connector={source.connector} className="w-5 h-5" />
            </span>
            <div className="min-w-0">
              <h1 className="m-0 text-[15px] font-semibold text-shodh-text truncate">{conversation.title}</h1>
              <p className="m-0 mt-0.5 flex items-center gap-1.5 text-[12px] text-shodh-text-muted">
                <span className="inline-flex items-center gap-1 h-5 px-1.5 rounded-md bg-shodh-raised text-[11px] font-medium text-shodh-text-secondary">
                  <Lock className="w-3 h-3" aria-hidden="true" />
                  From {name}
                </span>
                <span className="truncate">{source.label}</span>
              </p>
            </div>
          </div>
          <div className="flex items-center gap-2">
            {link && (
              <button
                type="button"
                className={OUTLINE_BUTTON}
                onClick={() => void openUrl(link).catch(() => notify.error(`${name} could not be opened`))}
              >
                <ExternalLink className="w-3.5 h-3.5" aria-hidden="true" />
                Open in {name}
              </button>
            )}
            <button type="button" className={PRIMARY_BUTTON} disabled={!canContinue} onClick={() => onContinue(conversation)}>
              <MessageSquarePlus className="w-4 h-4" aria-hidden="true" />
              Continue in Ask
            </button>
          </div>
        </div>
      </header>

      <div className="flex-1 overflow-y-auto scrollbar-thin">
        <ol className="max-w-[700px] mx-auto px-7 py-6 m-0 list-none flex flex-col gap-5" aria-label="Messages">
          {conversation.messages.map(message => {
            const author = messageAuthor(message);
            const when = time(message.timestamp);
            if (author.shodh) {
              return (
                <li key={message.id} className="flex flex-col gap-1">
                  <span className="text-[11.5px] font-medium text-shodh-text-muted">
                    Shodh{when && <span className="font-normal text-shodh-text-faint">{`, ${when}`}</span>}
                  </span>
                  <div className="text-[14.5px] leading-[1.6] text-shodh-text [&_p]:my-2 [&_ul]:my-2 [&_ul]:pl-5 [&_ol]:pl-5 [&_li]:my-0.5">
                    <ReactMarkdown remarkPlugins={[remarkGfm]} components={MARKDOWN_COMPONENTS}>
                      {message.content}
                    </ReactMarkdown>
                  </div>
                </li>
              );
            }
            return (
              <li key={message.id} className={cn('flex flex-col gap-1', author.owner ? 'items-end' : 'items-start')}>
                <span className="text-[11.5px] font-medium text-shodh-text-muted">
                  {author.owner ? 'You' : author.name}
                  {when && <span className="font-normal text-shodh-text-faint">{`, ${when}`}</span>}
                </span>
                <div
                  className={cn(
                    'max-w-[520px] min-w-0 px-4 py-[10px] rounded-[18px] text-[14.5px] leading-[1.55] whitespace-pre-wrap break-words',
                    author.owner ? 'bg-shodh-raised-2 text-shodh-text' : 'bg-shodh-surface border border-shodh-border text-shodh-text',
                  )}
                >
                  {message.content}
                </div>
              </li>
            );
          })}
        </ol>
      </div>

      <footer className="shrink-0 border-t border-shodh-border px-7 py-3">
        <p className="max-w-[700px] mx-auto m-0 text-[12px] text-shodh-text-muted" role="note">
          {readOnlyLine(source)}
        </p>
      </footer>
    </section>
  );
}

/**
 * Above a chat continued from a connector conversation, until its first question is
 * sent: what that question will carry, and a way to not carry it.
 */
export function SeedNotice({
  label,
  connector,
  messages,
  onRemove,
}: {
  label: string;
  connector: string;
  messages: number;
  onRemove: () => void;
}) {
  return (
    <div role="note" className="flex items-start gap-3 rounded-[14px] border border-shodh-border bg-shodh-surface px-4 py-3">
      <span className="w-7 h-7 shrink-0 rounded-lg bg-white border border-black/10 dark:border-white/10 inline-flex items-center justify-center" aria-hidden="true">
        <ConnectorIcon connector={connector} className="w-4 h-4" />
      </span>
      <div className="min-w-0 flex-1">
        <p className="m-0 text-[13px] font-medium text-shodh-text">Continuing {label}</p>
        <p className="m-0 mt-0.5 text-[12.5px] text-shodh-text-muted">
          Your first question carries its last {messages} {messages === 1 ? 'message' : 'messages'} as quoted context. Other people's words in it are never treated as instructions or saved to your memories.
        </p>
      </div>
      <button
        type="button"
        onClick={onRemove}
        className={cn('shrink-0 h-7 px-2 rounded-md text-[12px] text-shodh-text-muted hover:bg-shodh-raised hover:text-shodh-text', FOCUS_RING)}
      >
        Don't include
      </button>
    </div>
  );
}
