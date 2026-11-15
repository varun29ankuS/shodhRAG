/**
 * "Ask about this": text or a link another app shared to Shodh. The sheet
 * shows what was shared and takes the question; it is answered from the whole
 * library, and a shared link counts as one the user gave (lib/share.ts).
 */
import { useState, type FormEvent } from 'react';
import { ArrowUp, Link2, Quote } from 'lucide-react';
import { useLink, useNav, useNote } from '../lib/context';
import { useOffers, type Shared } from '../lib/incoming';
import { errorText } from '../lib/link';
import { queuedQuestion } from '../lib/outbox';
import { canQueue, useQueue } from '../lib/queue';
import { sharedLink, sharedQuestion } from '../lib/share';
import { Sheet } from './Sheet';

export function AskAbout({ shared }: { shared: Shared }) {
  const { connected, status } = useLink();
  const { dispatch } = useNav();
  const { closeAsk } = useOffers();
  const queue = useQueue();
  const { show } = useNote();
  const [question, setQuestion] = useState('');
  const link = sharedLink(shared.text);
  const queueable = canQueue(status);

  const submit = (event: FormEvent) => {
    event.preventDefault();
    const asked = sharedQuestion(question, shared.text);
    const conversationId = crypto.randomUUID();
    if (connected) {
      closeAsk();
      dispatch({ type: 'push', screen: { kind: 'answer', conversationId, question: asked.text, workspaceId: null, context: asked.context } });
      return;
    }
    if (!queueable) return;
    queue
      .add(queuedQuestion({ conversationId, text: asked.text, workspaceId: null, attached: asked.context.attached }))
      .then(() => {
        closeAsk();
        show('Saved on your phone. It is sent when your computer wakes.');
        dispatch({ type: 'push', screen: { kind: 'answer', conversationId, question: null, workspaceId: null } });
      })
      .catch(e => show(errorText(e), 'problem'));
  };

  return (
    <Sheet label="Ask about this" onClose={closeAsk}>
      <span className="approval-context">
        {link ? <Link2 aria-hidden="true" className="icon-small" /> : <Quote aria-hidden="true" className="icon-small" />} Ask Shodh about this
      </span>
      <div className="sheet-card">
        {shared.subject && <span className="sheet-card-title">{shared.subject}</span>}
        <span className="sheet-card-text">{shared.text.length > 600 ? `${shared.text.slice(0, 600)}…` : shared.text}</span>
        <span className="passage-label">
          {link ? 'A link you shared: Shodh may open it to answer.' : 'Shared from another app. Shodh answers from your library.'}
        </span>
      </div>
      <form className="askbox" onSubmit={submit}>
        <label htmlFor="ask-about" className="visually-hidden">
          Your question about it
        </label>
        <input
          id="ask-about"
          value={question}
          onChange={event => setQuestion(event.target.value)}
          placeholder="What does my library say about this?"
          enterKeyHint="send"
          autoComplete="off"
        />
        <button type="submit" className="send" aria-label="Ask" disabled={!(connected || queueable)}>
          <ArrowUp aria-hidden="true" className="icon" />
        </button>
      </form>
    </Sheet>
  );
}
