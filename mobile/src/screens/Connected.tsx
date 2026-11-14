import { useEffect, useRef, useState } from 'react';
import { Loader2, RefreshCw, Send, Square, TriangleAlert } from 'lucide-react';
import { askOnce, AskStopped } from '../lib/ask';
import { desktop, errorText, reconnect, type LinkStatus } from '../lib/link';

const WORDS: Record<string, string> = {
  connecting: 'Connecting to your computer…',
  connected: 'Connected to your computer',
  offline: 'Your computer is not reachable',
  update: 'Update Shodh',
};

/** The connection, and a test question that streams one answer. */
export function Connected({ status }: { status: LinkStatus }) {
  const connected = status.state === 'connected';
  const [question, setQuestion] = useState('');
  const [answer, setAnswer] = useState('');
  const [asking, setAsking] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [retrying, setRetrying] = useState(false);
  const stop = useRef<AbortController | null>(null);

  // A question stops waiting when the connection ends or the screen goes.
  useEffect(() => {
    if (!connected) stop.current?.abort();
  }, [connected]);
  useEffect(() => () => stop.current?.abort(), []);

  const ask = async () => {
    const controller = new AbortController();
    stop.current = controller;
    setAsking(true);
    setAnswer('');
    setError(null);
    try {
      const result = await askOnce(desktop, question.trim(), { onText: setAnswer, signal: controller.signal });
      setAnswer(result.text);
      if (result.status === 'error') setError(result.error ?? 'The answer stopped with an error.');
      if (result.status === 'aborted') setError('The answer was stopped.');
    } catch (e) {
      setError(e instanceof AskStopped ? 'The question was stopped. Your computer still saves the answer.' : errorText(e));
    } finally {
      if (stop.current === controller) stop.current = null;
      setAsking(false);
    }
  };

  const retry = () => {
    setRetrying(true);
    reconnect()
      .catch(e => setError(errorText(e)))
      .finally(() => setRetrying(false));
  };

  const message = 'message' in status ? status.message : null;

  return (
    <section className="screen">
      <p className="status" role="status">
        <span className={`dot dot-${status.state}`} aria-hidden="true" />
        {WORDS[status.state] ?? status.state}
      </p>
      {message && <p className="muted">{message}</p>}
      {status.state === 'offline' && (
        <button type="button" className="button" disabled={retrying} onClick={retry}>
          {retrying ? <Loader2 aria-hidden="true" className="icon spin" /> : <RefreshCw aria-hidden="true" className="icon" />}
          Try again
        </button>
      )}

      {connected && (
        <form
          className="ask"
          onSubmit={event => {
            event.preventDefault();
            if (!asking && question.trim()) void ask();
          }}
        >
          <label htmlFor="question">Ask your library</label>
          <textarea
            id="question"
            rows={3}
            value={question}
            onChange={event => setQuestion(event.target.value)}
            placeholder="What does my library say about…"
          />
          {asking ? (
            <button type="button" className="button" onClick={() => stop.current?.abort()}>
              <Square aria-hidden="true" className="icon" />
              Stop waiting
            </button>
          ) : (
            <button type="submit" className="button button-primary" disabled={question.trim() === ''}>
              <Send aria-hidden="true" className="icon" />
              Ask
            </button>
          )}
        </form>
      )}

      {(asking || answer) && (
        <article className="answer" aria-live="polite" aria-busy={asking}>
          {answer || <span className="muted">Your computer is answering…</span>}
        </article>
      )}
      {error && (
        <div className="callout callout-error" role="alert">
          <TriangleAlert aria-hidden="true" className="icon" />
          <p>{error}</p>
        </div>
      )}
    </section>
  );
}
