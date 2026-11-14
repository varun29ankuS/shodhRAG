/**
 * "Point the camera and ask": the photo just taken goes to the computer,
 * which reads it (its own OCR) and keeps it out of the library; the sheet
 * shows what was read and takes the question. The answer cites the photo as
 * "Your photo", next to the library.
 */
import { useEffect, useState, type FormEvent } from 'react';
import { ArrowUp, Camera, LoaderCircle } from 'lucide-react';
import { useNav } from '../lib/context';
import type { Picked, PhotoRead } from '../lib/device';
import { errorText } from '../lib/link';
import { photoContext } from '../lib/photo';
import { useDevice } from '../lib/queue';
import { Sheet } from './Sheet';

export function PhotoAsk({ taken, conversationId, onClose }: { taken: Picked; conversationId: string; onClose: () => void }) {
  const { files } = useDevice();
  const { dispatch } = useNav();
  const [read, setRead] = useState<PhotoRead | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [question, setQuestion] = useState('');

  useEffect(() => {
    let alive = true;
    files
      .details(taken, null)
      .then(file => files.sendPhoto(file))
      .then(result => {
        if (alive) setRead(result);
      })
      .catch(e => {
        if (alive) setError(errorText(e));
      });
    return () => {
      alive = false;
    };
  }, [files, taken]);

  const close = () => {
    files.discard(taken.path).catch(() => undefined);
    onClose();
  };

  const submit = (event: FormEvent) => {
    event.preventDefault();
    const text = question.trim();
    if (!read || !text) return;
    onClose();
    dispatch({ type: 'push', screen: { kind: 'answer', conversationId, question: text, workspaceId: null, context: photoContext(read) } });
  };

  return (
    <Sheet label="Ask about your photo" onClose={close}>
      <span className="approval-context">
        <Camera aria-hidden="true" className="icon-small" /> Ask about your photo
      </span>
      <div className="sheet-card">
        <span className="sheet-card-title serif">Your photo</span>
        {read === null && error === null && (
          <span className="sheet-card-text" role="status">
            <LoaderCircle aria-hidden="true" className="icon-small spin" /> Your computer is reading it…
          </span>
        )}
        {read !== null && (
          <span className="sheet-card-text serif">{read.text ? read.text.slice(0, 400) : 'Shodh found no text in this photo. Ask about what it shows anyway, or take another.'}</span>
        )}
        {error !== null && <span className="sheet-card-text">{error}</span>}
        <span className="passage-label">Not added to your library unless you keep it.</span>
      </div>
      <form className="askbox" onSubmit={submit}>
        <label htmlFor="photo-question" className="visually-hidden">
          Your question about the photo
        </label>
        <input
          id="photo-question"
          value={question}
          onChange={event => setQuestion(event.target.value)}
          placeholder="What do you want to know about it?"
          enterKeyHint="send"
          autoComplete="off"
        />
        <button type="submit" className="send" aria-label="Ask" disabled={read === null || question.trim() === ''}>
          <ArrowUp aria-hidden="true" className="icon" />
        </button>
      </form>
    </Sheet>
  );
}
