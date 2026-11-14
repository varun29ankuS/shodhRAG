/**
 * Ask's first screen: a greeting, the ask box, Today, the conversation to
 * continue, questions about the user's own folders, and recent
 * conversations. Everything comes from the computer.
 */
import { useState, type FormEvent } from 'react';
import { ArrowUp, CalendarDays, Camera, Sparkles } from 'lucide-react';
import { AnimatePresence, motion } from 'framer-motion';
import { PhotoAsk } from '../components/PhotoAsk';
import type { Picked } from '../lib/device';
import { TopBar } from '../components/Bars';
import { connectionWords, NotConnected } from '../components/Connection';
import { PageThumb } from '../components/PageThumb';
import { answerText, citedNumbers, savedAnswer } from '../lib/answer';
import { useLink, useNav, useNote } from '../lib/context';
import { useData } from '../lib/data';
import { libraryFolders, loadConversation, type ConversationSummary } from '../lib/desktop';
import { snippet } from '../lib/markdown';
import { viewIn } from '../lib/motion';
import { useRemote } from '../lib/remote';
import { ago, dayName, greeting } from '../lib/time';
import { QueuedRows } from '../components/Queued';
import { useAgenda } from '../lib/agenda';
import { agendaItems, dayKey, timeWords } from './Tasks';
import { errorText } from '../lib/link';
import { queuedQuestion } from '../lib/outbox';
import { canQueue, useDevice, useQueue } from '../lib/queue';

/** A new conversation's id, as the desktop makes them. */
export function newConversationId(): string {
  return crypto.randomUUID();
}

export function Home({ now = () => new Date() }: { now?: () => Date }) {
  const { status, connected } = useLink();
  const { dispatch } = useNav();
  const { conversations } = useData();
  const queue = useQueue();
  const { show } = useNote();
  const queueable = canQueue(status);
  const [question, setQuestion] = useState('');
  const { files } = useDevice();
  const [photo, setPhoto] = useState<{ taken: Picked; conversationId: string } | null>(null);
  const takePhoto = () =>
    files
      .takePhoto()
      .then(taken => {
        if (taken) setPhoto({ taken, conversationId: newConversationId() });
      })
      .catch(e => show(errorText(e), 'problem'));
  const today = now();
  const words = connectionWords(status);

  const ask = (text: string) => {
    const trimmed = text.trim();
    if (!trimmed) return;
    if (connected) {
      dispatch({ type: 'push', screen: { kind: 'answer', conversationId: newConversationId(), question: trimmed, workspaceId: null } });
      setQuestion('');
      return;
    }
    if (!queueable) return;
    // The computer is asleep: the question waits on the phone and is sent when it wakes.
    const conversationId = newConversationId();
    queue
      .add(queuedQuestion({ conversationId, text: trimmed, workspaceId: null }))
      .then(() => {
        setQuestion('');
        show('Saved on your phone. It is sent when your computer wakes.');
        dispatch({ type: 'push', screen: { kind: 'answer', conversationId, question: null, workspaceId: null } });
      })
      .catch(e => show(errorText(e), 'problem'));
  };
  const waiting = queue.items.filter(i => i.state !== 'answered');
  const submit = (event: FormEvent) => {
    event.preventDefault();
    ask(question);
  };

  // The latest first: the one to continue, then the recent ones.
  const list = conversations.data ? [...conversations.data].sort((a, b) => b.updatedAt.localeCompare(a.updatedAt)) : null;

  return (
    <motion.div className="view" variants={viewIn} initial="hidden" animate="shown" exit="gone">
      <div className="scroll">
        <TopBar />
        <div className="pad" style={{ paddingTop: 26 }}>
          <h1 className="greeting serif">{greeting(today)}</h1>
          <p className={`line${words.dot === 'waiting' && status.state === 'offline' ? ' line-waiting' : ''}`} style={{ marginTop: 8 }}>
            {words.line}
          </p>
        </div>

        <form className="askbox" style={{ margin: '22px 16px 0' }} onSubmit={submit}>
          <label htmlFor="home-ask" className="visually-hidden">
            Ask about your library
          </label>
          <input
            id="home-ask"
            value={question}
            onChange={event => setQuestion(event.target.value)}
            placeholder={connected ? 'Ask about your library' : queueable ? 'Ask now. It is sent when your computer wakes' : 'Connect to your computer to ask'}
            enterKeyHint="send"
            autoComplete="off"
          />
          <button type="button" className="send send-quiet" aria-label="Ask about a photo" disabled={!connected} onClick={takePhoto}>
            <Camera aria-hidden="true" className="icon" />
          </button>
          <button type="submit" className="send" aria-label="Send" disabled={!(connected || queueable) || question.trim() === ''}>
            <ArrowUp aria-hidden="true" className="icon" />
          </button>
        </form>
        <AnimatePresence>
          {photo && <PhotoAsk key="photo" taken={photo.taken} conversationId={photo.conversationId} onClose={() => setPhoto(null)} />}
        </AnimatePresence>

        {waiting.length > 0 && (
          <>
            <div className="label">Waiting for your computer</div>
            <QueuedRows items={waiting} />
          </>
        )}

        <div className="label">
          <span>Today</span>
          <span className="label-note">{dayName(today)}</span>
        </div>
        <TodayCard today={today} />

        {list === null && !connected ? (
          <NotConnected what="your conversations" />
        ) : list === null ? (
          <HomeSkeleton />
        ) : (
          <>
            {list.length > 0 && <Continue conversation={list[0]} now={today} />}
            <Suggestions onAsk={ask} />
            <Recent list={list.slice(1, 4)} empty={list.length === 0} now={today} />
          </>
        )}
      </div>
    </motion.div>
  );
}

function HomeSkeleton() {
  return (
    <div aria-busy="true" aria-label="Loading your conversations">
      <div className="label">Continue</div>
      <div className="skeleton" style={{ margin: '0 16px', height: 120, borderRadius: 22 }} />
      <div className="skeleton-lines">
        <div className="skeleton skeleton-line" style={{ width: '82%' }} />
        <div className="skeleton skeleton-line" style={{ width: '64%' }} />
      </div>
    </div>
  );
}

/** The latest conversation: its question, the start of its answer, and the page it cites first. */
function Continue({ conversation, now }: { conversation: ConversationSummary; now: Date }) {
  const { dispatch } = useNav();
  const saved = useRemote(`conversation:${conversation.id}:${conversation.updatedAt}`, desktop => loadConversation(desktop, conversation.id));
  const messages = saved.data?.messages ?? [];
  const lastQuestion = [...messages].reverse().find(m => m.role === 'user');
  const lastAnswer = [...messages].reverse().find(m => m.role === 'assistant');
  const answer = lastAnswer ? savedAnswer(lastAnswer.content, lastAnswer.transcript) : null;
  const text = answer ? answerText(answer) : '';
  const firstCited = answer
    ? citedNumbers(text)
        .map(n => answer.passages.find(p => p.n === n && !p.web))
        .find(p => p !== undefined)
    : undefined;
  const open = () =>
    dispatch({ type: 'push', screen: { kind: 'answer', conversationId: conversation.id, question: null, workspaceId: conversation.workspaceId } });
  return (
    <>
      <div className="label">Continue</div>
      <button type="button" className="continue" onClick={open}>
        <span className="row-text" style={{ gap: 6 }}>
          <span className="continue-question serif">{lastQuestion?.content ?? conversation.title}</span>
          {text !== '' && <span className="continue-answer">{snippet(text, 140)}</span>}
          <span className="row-sub">{ago(conversation.updatedAt, now)}</span>
        </span>
        {firstCited && (
          <span style={{ width: 54, flex: 'none' }}>
            <PageThumb path={firstCited.path} page={firstCited.page} />
          </span>
        )}
      </button>
    </>
  );
}

/** Questions about the user's own linked folders. */
function Suggestions({ onAsk }: { onAsk: (question: string) => void }) {
  const roots = useRemote('library:roots', desktop => libraryFolders(desktop, null));
  const folders = (roots.data?.folders ?? []).slice(0, 3);
  if (folders.length === 0) return null;
  return (
    <>
      <div className="label">Ask about your library</div>
      <div className="rows">
        {folders.map(folder => {
          const question = `What do my files in ${folder.name} cover?`;
          return (
            <button key={folder.path} type="button" className="row" onClick={() => onAsk(question)}>
              <Sparkles aria-hidden="true" className="icon" style={{ color: 'var(--c-accent-text)' }} />
              <span className="row-title-serif" style={{ flex: 1 }}>
                {question}
              </span>
            </button>
          );
        })}
      </div>
    </>
  );
}

function Recent({ list, empty, now }: { list: ConversationSummary[]; empty: boolean; now: Date }) {
  const { dispatch } = useNav();
  return (
    <>
      <div className="label">
        <span>Recent</span>
        {!empty && (
          <button type="button" className="link-button" onClick={() => dispatch({ type: 'push', screen: { kind: 'conversations' } })}>
            See all
          </button>
        )}
      </div>
      {empty ? (
        <p className="panel-text pad">No conversations yet. Ask a question above to start one.</p>
      ) : (
        <div className="rows">
          {list.map(c => (
            <button
              key={c.id}
              type="button"
              className="row"
              onClick={() => dispatch({ type: 'push', screen: { kind: 'answer', conversationId: c.id, question: null, workspaceId: c.workspaceId } })}
            >
              <span className="row-text">
                <span className="row-title">{c.title}</span>
                <span className="row-sub">{ago(c.updatedAt, now)}</span>
              </span>
            </button>
          ))}
        </div>
      )}
    </>
  );
}

/** Today: the next things on the agenda and what is due, from the computer (or its copy). */
function TodayCard({ today }: { today: Date }) {
  const { dispatch } = useNav();
  const agenda = useAgenda();
  const items = agenda.events ? agendaItems(agenda.events, agenda.phoneEvents, today, 1).slice(0, 3) : [];
  const due = (agenda.tasks ?? []).filter(t => t.status !== 'completed' && t.dueDate?.slice(0, 10) === dayKey(today)).length;
  const summary =
    agenda.events === null
      ? 'Your agenda and tasks show when your computer answers.'
      : items.length === 0
        ? due === 0
          ? 'Nothing on your calendar today.'
          : `Nothing on your calendar today; ${due} ${due === 1 ? 'task is' : 'tasks are'} due.`
        : due === 0
          ? `${items.length} on your calendar today.`
          : `${items.length} on your calendar today, and ${due} ${due === 1 ? 'task' : 'tasks'} due.`;
  return (
    <div className="card">
      <button type="button" className="row row-tinted" style={{ borderRadius: 0, width: '100%' }} onClick={() => dispatch({ type: 'go', section: 'tasks' })}>
        <span className="row-icon">
          <CalendarDays aria-hidden="true" className="icon-small" />
        </span>
        <span className="row-text">
          <span className="row-title">{items[0] ? `${timeWords(items[0].fields)} \u00b7 ${items[0].title}` : 'Your agenda and tasks'}</span>
          <span className="row-sub">{summary}</span>
        </span>
      </button>
      {items.slice(1).map(item => (
        <div key={item.key} className="row" style={{ borderRadius: 0 }}>
          <span className="row-text">
            <span className="row-title">{`${timeWords(item.fields)} \u00b7 ${item.title}`}</span>
          </span>
        </div>
      ))}
    </div>
  );
}
