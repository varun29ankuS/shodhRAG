/**
 * A conversation: each question as a headline and its answer under it, as
 * the computer saved them, and a new question's answer as it streams. Ask a
 * follow-up from the box at the bottom; Stop ends the answer being written.
 *
 * The layout follows the conversation's kind on the computer (lib/modes.ts):
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
import { ArrowUp, Square, TriangleAlert } from 'lucide-react';
import { AnswerBlocks, AnswerSources, whereOf, type CiteHandler } from '../components/AnswerView';
import { BackBar } from '../components/Bars';
import { CodeNote, CodeWorkView, CreatorPreview } from '../components/CodeWork';
import { connectionWords, Dot, NotConnected } from '../components/Connection';
import { Sheet } from '../components/Sheet';
import { answerText, citedNumbers, isLive, savedAnswer, startingAnswer, type AnswerState, type Passage } from '../lib/answer';
import { ask, AskStopped } from '../lib/ask';
import { codeWork, NO_WORK, type CodeWork } from '../lib/code';
import { useLink, useNav } from '../lib/context';
import { abortAnswer, CONVERSATION_CHANGED, loadConversation, type HistoryTurn, type SavedConversation } from '../lib/desktop';
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

function historyOf(conversation: SavedConversation | null | undefined): HistoryTurn[] {
  return (conversation?.messages ?? [])
    .filter((m): m is SavedConversation['messages'][number] & { role: 'user' | 'assistant' } => m.role === 'user' || m.role === 'assistant')
    .map(m => ({ role: m.role, content: m.role === 'assistant' ? answerText(savedAnswer(m.content, m.transcript)) : m.content }))
    .filter(t => t.content.trim() !== '');
}

interface Live {
  question: string;
  answer: AnswerState;
  sessionId: string | null;
  /** The phone stopped waiting (the connection ended, or it was stopped). */
  stopped: string | null;
}

export function Ask({ conversationId, question, workspaceId }: { conversationId: string; question: string | null; workspaceId: string | null }) {
  const { desktop, subscriptions, connected, status } = useLink();
  const { dispatch } = useNav();
  const saved = useRemote(`conversation:${conversationId}`, d => loadConversation(d, conversationId), [CONVERSATION_CHANGED]);
  const [live, setLive] = useState<Live | null>(null);
  const [followUp, setFollowUp] = useState('');
  const [web, setWeb] = useState<Passage | null>(null);
  const stop = useRef<AbortController | null>(null);
  const asked = useRef(false);
  const liveRef = useRef<HTMLDivElement>(null);

  const start = useCallback(
    (text: string, history: HistoryTurn[]) => {
      stop.current?.abort();
      const controller = new AbortController();
      stop.current = controller;
      setLive({ question: text, answer: startingAnswer(), sessionId: null, stopped: null });
      ask(desktop, subscriptions, { conversationId, text, history, workspaceId }, {
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
    start(question, []);
  }, [question, connected, start]);

  // The phone stops waiting when the connection ends or the screen goes.
  useEffect(() => {
    if (!connected) stop.current?.abort();
  }, [connected]);
  useEffect(() => () => stop.current?.abort(), []);

  useEffect(() => {
    if (live && live.answer.status === 'starting') liveRef.current?.scrollIntoView({ block: 'start' });
  }, [live]);

  const turns = savedTurns(saved.data);
  // The live answer, until the saved conversation holds it.
  const lastSaved = turns[turns.length - 1];
  const liveSaved =
    live !== null && !isLive(live.answer) && lastSaved !== undefined && lastSaved.question === live.question && answerText(lastSaved.answer) !== '';
  const showLive = live !== null && !liveSaved;
  const running = live !== null && isLive(live.answer) && live.stopped === null;

  const onCite: CiteHandler = (passage, origin) => {
    if (passage.web) {
      setWeb(passage);
      return;
    }
    dispatch({ type: 'read', target: { path: passage.path, name: passage.title, page: passage.page, passage: passage.text || null, back: 'Answer', origin } });
  };

  const submit = (event: FormEvent) => {
    event.preventDefault();
    const text = followUp.trim();
    if (!text || !connected || running) return;
    setFollowUp('');
    start(text, historyOf(saved.data));
  };

  const words = connectionWords(status);
  // Older records have no mode: Research, the reading layout.
  const kind = saved.data ? conversationKind(saved.data) : 'research';
  return (
    <motion.div className="view" variants={pushIn} initial="hidden" animate="shown" exit="gone">
      <BackBar label="Ask">
        <span className="connection" style={{ paddingRight: 8 }}>
          <Dot status={status} small />
          {words.short}
        </span>
      </BackBar>
      <div className={kind === 'code' ? 'scroll layout-code' : 'scroll'} data-layout={kind}>
        {kind === 'code' && <CodeNote />}
        {kind === 'creator' && <CreatorPreview work={creationWork(turns)} />}
        {saved.data === undefined && !showLive && !connected && <NotConnected what="this conversation" />}
        {saved.data === undefined && !showLive && connected && (
          <div className="skeleton-lines" aria-busy="true" aria-label="Loading the conversation">
            <div className="skeleton skeleton-line" style={{ width: '90%', height: 28 }} />
            <div className="skeleton skeleton-line" style={{ width: '94%' }} />
            <div className="skeleton skeleton-line" style={{ width: '70%' }} />
          </div>
        )}
        {saved.data === null && !showLive && <p className="panel-text pad">This conversation is no longer on your computer.</p>}
        {turns.map(turn => (
          <section key={turn.id} className="turn">
            {turn.question !== '' && <h1 className={kind === 'code' ? 'question mono' : 'question serif'}>{turn.question}</h1>}
            <TurnStatus answer={turn.answer} />
            <AnswerBlocks answer={turn.answer} onCite={onCite} />
            {kind === 'code' && <CodeWorkView work={turn.work} />}
            <AnswerSources answer={turn.answer} onCite={onCite} />
          </section>
        ))}
        {showLive && live && (
          <section className="turn" ref={liveRef}>
            <h1 className={kind === 'code' ? 'question mono' : 'question serif'}>{live.question}</h1>
            <TurnStatus answer={live.answer} />
            {live.stopped !== null && (
              <div className="callout" role="status" style={{ margin: '18px 24px 0' }}>
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
                  <span className="approval-title serif">{step.label}</span>
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
          placeholder={!connected ? 'Connect to your computer to ask' : kind === 'code' ? 'Ask a question about this code' : 'Ask a follow-up'}
          enterKeyHint="send"
          autoComplete="off"
        />
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
          <button type="submit" className="send" aria-label="Send" disabled={!connected || followUp.trim() === ''}>
            <ArrowUp aria-hidden="true" className="icon" />
          </button>
        )}
      </form>

      <AnimatePresence>
        {web && (
          <Sheet label="Web source" onClose={() => setWeb(null)}>
            <span className="approval-context">{`Source ${web.n}, from the web`}</span>
            <div className="sheet-card">
              <span className="sheet-card-title serif">{web.title}</span>
              {web.text && <span className="sheet-card-text serif">{web.text}</span>}
              <span className="passage-label">{whereOf(web)}</span>
            </div>
            <button type="button" className="button button-wide" onClick={() => setWeb(null)}>
              Close
            </button>
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
