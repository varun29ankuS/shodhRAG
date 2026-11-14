/**
 * A conversation: each question as a headline and its answer under it, as
 * the computer saved them, and a new question's answer as it streams. Ask a
 * follow-up from the box at the bottom; Stop ends the answer being written.
 *
 * How it reads follows the conversation's kind on the computer (lib/modes.ts):
 * Research reads as prose; Code is monospace, with the files each answer
 * changed and the commands it ran, from the saved transcript, under a note
 * that code changes are approved on the computer; Creator says where to see
 * its preview. A question from the phone always starts a Research session
 * (it sends no mode, and the computer refuses Code from a phone), so in a
 * Code conversation the phone asks questions and anything that edits waits
 * for the computer.
 */
import { useCallback, useEffect, useRef, useState, type FormEvent } from 'react';
import { AnimatePresence, motion } from 'framer-motion';
import { ArrowUp, Camera, LibraryBig, Square, TriangleAlert } from 'lucide-react';
import { AnswerBlocks, AnswerSources, whereOf, type CiteHandler } from '../components/AnswerView';
import { BackBar } from '../components/Bars';
import { CodeNote, CodeWorkView, CreatorPreview } from '../components/CodeWork';
import { connectionWords, Dot, NotConnected } from '../components/Connection';
import { Sheet } from '../components/Sheet';
import { answerText, citedNumbers, isLive, savedAnswer, startingAnswer, type AnswerState, type Passage } from '../lib/answer';
import { ask, asPassage, AskStopped, historyOf } from '../lib/ask';
import type { AskContext } from '../lib/nav';
import { isPhotoPath, photoIdOf } from '../lib/photo';
import { codeWork, NO_WORK, type CodeWork } from '../lib/code';
import { useLink, useNav, useNote } from '../lib/context';
import { QueuedRows } from '../components/Queued';
import { MicButton, ReadAloud } from '../components/Voice';
import { queuedQuestion } from '../lib/outbox';
import { canQueue, useQueue } from '../lib/queue';
import { abortAnswer, CONVERSATION_CHANGED, loadConversation, workspacesList, type HistoryTurn, type SavedConversation } from '../lib/desktop';
import { errorText } from '../lib/link';
import { conversationKind } from '../lib/modes';
import { pushIn } from '../lib/motion';
import { useRemote } from '../lib/remote';

interface Turn {
  id: string;
  question: string;
  answer: AnswerState;
  /** What the answer changed and ran (Code and Creator), from its transcript. */
  work: CodeWork;
}

/** The saved conversation as question and answer pairs. */
export function savedTurns(conversation: SavedConversation | null | undefined): Turn[] {
  const turns: Turn[] = [];
  for (const message of conversation?.messages ?? []) {
    if (message.role === 'user') {
      turns.push({ id: message.id, question: message.content, answer: { ...startingAnswer(), status: 'completed' }, work: NO_WORK });
    } else if (message.role === 'assistant') {
      const answer = savedAnswer(message.content, message.transcript);
      const work = codeWork(message.transcript);
      const last = turns[turns.length - 1];
      if (last && answerText(last.answer) === '' && last.answer.passages.length === 0 && last.work === NO_WORK) {
        last.answer = answer;
        last.work = work;
      } else turns.push({ id: message.id, question: '', answer, work });
    }
  }
  return turns;
}

/** A Creator conversation's build: every file its answers made, and the latest summary it finished with. */
export function creationWork(turns: Turn[]): CodeWork {
  const files = new Map<string, CodeWork['files'][number]>();
  let finished: string | null = null;
  for (const turn of turns) {
    for (const file of turn.work.files) files.set(file.path, file);
    finished = turn.work.finished ?? finished;
  }
  return { ...NO_WORK, files: [...files.values()], finished };
}

interface Live {
  question: string;
  answer: AnswerState;
  sessionId: string | null;
  /** The phone stopped waiting (the connection ended, or it was stopped). */
  stopped: string | null;
}

export function Ask({
  conversationId,
  question,
  workspaceId,
  context,
}: {
  conversationId: string;
  question: string | null;
  workspaceId: string | null;
  /** What the first question is about besides its words (a photo, shared text). */
  context?: AskContext;
}) {
  const { desktop, subscriptions, connected, status } = useLink();
  const { dispatch } = useNav();
  const queue = useQueue();
  const { show } = useNote();
  const queueable = canQueue(status);
  const saved = useRemote(`conversation:${conversationId}`, d => loadConversation(d, conversationId), [CONVERSATION_CHANGED]);
  // The workspace's name heads the screen; none for a conversation outside a workspace, or one not read yet.
  const workspaces = useRemote('workspaces', workspacesList);
  const workspaceName = workspaceId === null ? null : (workspaces.data?.find(w => w.id === workspaceId)?.name ?? null);
  const [live, setLive] = useState<Live | null>(null);
  const [followUp, setFollowUp] = useState('');
  const [web, setWeb] = useState<Passage | null>(null);
  const [photo, setPhoto] = useState<Passage | null>(null);
  const [kept, setKept] = useState<string | null>(null);
  const keep = (photoId: string) => {
    desktop
      .invoke<{ added: boolean; reason: string | null }>('phone_photo_keep', { photoId })
      .then(answer => setKept(answer.added ? 'Added to your library, in From my phone.' : `Not added: ${answer.reason ?? 'your computer did not add it.'}`))
      .catch(e => setKept(errorText(e)));
  };
  const stop = useRef<AbortController | null>(null);
  const asked = useRef(false);
  const liveRef = useRef<HTMLDivElement>(null);

  const start = useCallback(
    (text: string, history: HistoryTurn[], about?: AskContext) => {
      stop.current?.abort();
      const controller = new AbortController();
      stop.current = controller;
      setLive({ question: text, answer: startingAnswer(), sessionId: null, stopped: null });
      ask(desktop, subscriptions, { conversationId, text, history, workspaceId, attached: about?.attached, passages: about?.passages }, {
        signal: controller.signal,
        onSession: sessionId => setLive(l => (l && l.question === text ? { ...l, sessionId } : l)),
        onAnswer: answer => setLive(l => (l && l.question === text ? { ...l, answer } : l)),
      })
        .then(answer => setLive(l => (l && l.question === text ? { ...l, answer } : l)))
        .catch(error => {
          const words =
            error instanceof AskStopped
              ? 'Your phone stopped waiting. Your computer keeps the answer; it shows here when your computer is back.'
              : errorText(error);
          setLive(l => (l && l.question === text ? { ...l, stopped: words } : l));
        })
        .finally(() => {
          if (stop.current === controller) stop.current = null;
        });
    },
    [desktop, subscriptions, conversationId, workspaceId],
  );

  // A new question is asked once, when the screen opens.
  useEffect(() => {
    if (question === null || asked.current || !connected) return;
    asked.current = true;
    start(question, [], context);
  }, [question, connected, start, context]);

  // The phone stops waiting when the connection ends or the screen goes.
  useEffect(() => {
    if (!connected) stop.current?.abort();
  }, [connected]);
  useEffect(() => () => stop.current?.abort(), []);

  useEffect(() => {
    if (live && live.answer.status === 'starting') liveRef.current?.scrollIntoView({ block: 'start' });
  }, [live]);

  // The first question's own passages ("Your photo") stay its sources once it is saved.
  const carried = (context?.passages ?? []).map(asPassage);
  const turns = savedTurns(saved.data).map(turn =>
    carried.length > 0 && turn.question === question
      ? { ...turn, answer: { ...turn.answer, passages: [...carried.filter(c => !turn.answer.passages.some(p => p.n === c.n)), ...turn.answer.passages] } }
      : turn,
  );
  // The live answer, until the saved conversation holds it.
  const lastSaved = turns[turns.length - 1];
  const liveSaved =
    live !== null && !isLive(live.answer) && lastSaved !== undefined && lastSaved.question === live.question && answerText(lastSaved.answer) !== '';
  const showLive = live !== null && !liveSaved;
  const running = live !== null && isLive(live.answer) && live.stopped === null;

  const onCite: CiteHandler = (passage, origin) => {
    if (isPhotoPath(passage.path)) {
      // The photo is on the phone, not in the library: its text, and "Keep in library".
      setPhoto(passage);
      return;
    }
    if (passage.web) {
      setWeb(passage);
      return;
    }
    dispatch({ type: 'read', target: { path: passage.path, name: passage.title, page: passage.page, passage: passage.text || null, back: 'Answer', origin } });
  };

  const submit = (event: FormEvent) => {
    event.preventDefault();
    const text = followUp.trim();
    if (!text || running) return;
    if (connected) {
      setFollowUp('');
      start(text, historyOf(saved.data));
      return;
    }
    if (!queueable) return;
    // The computer is asleep: kept on the phone, sent in order when it wakes.
    queue
      .add(queuedQuestion({ conversationId, text, workspaceId }))
      .then(() => setFollowUp(''))
      .catch(e => show(errorText(e), 'problem'));
  };

  // This conversation's questions waiting for the computer, until the saved conversation holds them.
  const savedIds = new Set((saved.data?.messages ?? []).map(m => m.id));
  const queuedHere = queue.items.filter(
    i => i.work.kind === 'question' && i.work.conversationId === conversationId && i.state !== 'answered' && !savedIds.has(i.work.questionId),
  );

  const words = connectionWords(status);
  // Older records have no mode: Research, the reading layout.
  const kind = saved.data ? conversationKind(saved.data) : 'research';
  return (
    <motion.div className="view" variants={pushIn} initial="hidden" animate="shown" exit="gone">
      <BackBar label="Ask" title={workspaceName}>
        <span className="connection" style={{ paddingRight: 12 }}>
          <Dot status={status} small />
          {words.short}
        </span>
      </BackBar>
      <div className={kind === 'code' ? 'scroll scroll-with-composer layout-code' : 'scroll scroll-with-composer'} data-layout={kind}>
        {kind === 'code' && <CodeNote />}
        {kind === 'creator' && <CreatorPreview work={creationWork(turns)} />}
        {saved.data === undefined && !showLive && !connected && queuedHere.length === 0 && <NotConnected what="this conversation" />}
        {saved.data === undefined && !showLive && connected && (
          <div className="skeleton-lines" aria-busy="true" aria-label="Loading the conversation">
            <div className="skeleton skeleton-line" style={{ width: '90%', height: 28 }} />
            <div className="skeleton skeleton-line" style={{ width: '94%' }} />
            <div className="skeleton skeleton-line" style={{ width: '70%' }} />
          </div>
        )}
        {saved.data === null && !showLive && <p className="panel-text panel-quiet">This conversation is no longer on your computer.</p>}
        {turns.map(turn => (
          <section key={turn.id} className="turn">
            {turn.question !== '' && <h1 className="question">{turn.question}</h1>}
            <TurnStatus answer={turn.answer} />
            <AnswerBlocks answer={turn.answer} onCite={onCite} />
            {kind === 'code' && <CodeWorkView work={turn.work} />}
            <AnswerSources answer={turn.answer} onCite={onCite} />
            <ReadAloud answer={turn.answer} />
          </section>
        ))}
        {queuedHere.length > 0 && (
          <section className="turn">
            <div className="label">Waiting for your computer</div>
            <QueuedRows items={queuedHere} />
          </section>
        )}
        {showLive && live && (
          <section className="turn" ref={liveRef}>
            <h1 className="question">{live.question}</h1>
            <TurnStatus answer={live.answer} />
            {live.stopped !== null && (
              <div className="callout" role="status" style={{ margin: '16px 20px 0' }}>
                <TriangleAlert aria-hidden="true" className="icon" />
                <p>{live.stopped}</p>
              </div>
            )}
            {isLive(live.answer) && live.stopped === null && answerText(live.answer) === '' && (
              <div className="skeleton-lines" aria-hidden="true">
                <div className="skeleton skeleton-line" style={{ width: '94%' }} />
                <div className="skeleton skeleton-line" style={{ width: '82%' }} />
                <div className="skeleton skeleton-line" style={{ width: '60%' }} />
              </div>
            )}
            <AnswerBlocks answer={live.answer} onCite={onCite} />
            {live.sessionId !== null &&
              live.answer.waiting.map(step => (
                <div key={step.stepId} className="waiting">
                  <span className="approval-context">The assistant is waiting for your answer</span>
                  <span className="approval-title">{step.label}</span>
                  <button
                    type="button"
                    className="button button-primary"
                    onClick={() => live.sessionId && dispatch({ type: 'sheet', target: { sessionId: live.sessionId, stepId: step.stepId } })}
                  >
                    Review
                  </button>
                </div>
              ))}
            <AnswerSources answer={live.answer} onCite={onCite} />
            <ReadAloud answer={live.answer} />
          </section>
        )}
      </div>

      <form className="askbox askbox-floating" onSubmit={submit}>
        <label htmlFor="follow-up" className="visually-hidden">
          Ask a follow-up
        </label>
        <input
          id="follow-up"
          value={followUp}
          onChange={event => setFollowUp(event.target.value)}
          placeholder={
            connected ? (kind === 'code' ? 'Ask a question about this code' : 'Ask a follow-up') : queueable ? 'Ask now. It is sent when your computer wakes' : 'Connect to your computer to ask'
          }
          enterKeyHint="send"
          autoComplete="off"
        />
        <MicButton onHeard={text => setFollowUp(text)} disabled={running} />
        {running ? (
          <button
            type="button"
            className="send"
            aria-label="Stop the answer"
            onClick={() => {
              if (live?.sessionId) abortAnswer(desktop, live.sessionId).catch(() => undefined);
            }}
          >
            <Square aria-hidden="true" className="icon-small" />
          </button>
        ) : (
          <button type="submit" className="send" aria-label="Send" disabled={!(connected || queueable) || followUp.trim() === ''}>
            <ArrowUp aria-hidden="true" className="icon" />
          </button>
        )}
      </form>

      <AnimatePresence>
        {web && (
          <Sheet label="Web source" onClose={() => setWeb(null)}>
            <span className="approval-context">{`Source ${web.n}, from the web`}</span>
            <div className="sheet-card">
              <span className="sheet-card-title">{web.title}</span>
              {web.text && <span className="sheet-card-text">{web.text}</span>}
              <span className="passage-label">{whereOf(web)}</span>
            </div>
            <button type="button" className="button button-wide" onClick={() => setWeb(null)}>
              Close
            </button>
          </Sheet>
        )}
        {photo && (
          <Sheet label="Your photo" onClose={() => setPhoto(null)}>
            <span className="approval-context">
              <Camera aria-hidden="true" className="icon-small" /> {`Source ${photo.n}, the photo you took`}
            </span>
            <div className="sheet-card">
              <span className="sheet-card-title">Your photo</span>
              {photo.text && <span className="sheet-card-text">{photo.text}</span>}
              <span className="passage-label">Read by Shodh on your computer. It is not in your library.</span>
            </div>
            {kept && <p className="panel-text">{kept}</p>}
            <div className="sheet-actions">
              <button type="button" className="button" onClick={() => setPhoto(null)}>
                Close
              </button>
              <button
                type="button"
                className="button button-primary"
                disabled={!connected || kept !== null}
                onClick={() => {
                  const id = photoIdOf(photo.path);
                  if (id) keep(id);
                }}
              >
                <LibraryBig aria-hidden="true" className="icon-small" />
                Keep in library
              </button>
            </div>
          </Sheet>
        )}
      </AnimatePresence>
    </motion.div>
  );
}

/** What the answer is doing, or where it came from. */
function TurnStatus({ answer }: { answer: AnswerState }) {
  let words: string;
  if (isLive(answer)) words = answer.doing ?? (answerText(answer) === '' ? 'Reading your library' : 'Writing the answer');
  else if (answer.status === 'error') words = answer.error ?? 'The answer stopped with an error.';
  else if (answer.status === 'aborted') words = answer.error ?? 'The answer was stopped.';
  else {
    const cited = citedNumbers(answerText(answer)).filter(n => answer.passages.some(p => p.n === n && !p.web)).length;
    words = cited === 0 ? '' : `Answered from ${cited} ${cited === 1 ? 'passage' : 'passages'} in your library`;
  }
  if (words === '') return null;
  return (
    <p className="status" role={isLive(answer) ? 'status' : undefined}>
      {words}
    </p>
  );
}
