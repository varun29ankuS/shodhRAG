/**
 * Voice: Talk (the phone's own speech recognition, on the device when the
 * phone has it) puts what the user said in the ask box; Read aloud says an
 * answer with its sources named from the citation metadata (lib/speech.ts).
 */
import { useState } from 'react';
import { Mic, MicOff, Square, Volume2 } from 'lucide-react';
import type { AnswerState } from '../lib/answer';
import { isLive } from '../lib/answer';
import { useNote } from '../lib/context';
import { errorText } from '../lib/link';
import { useDevice } from '../lib/queue';
import { spokenAnswer } from '../lib/speech';

export function MicButton({ onHeard, disabled = false }: { onHeard: (text: string) => void; disabled?: boolean }) {
  const { voice } = useDevice();
  const { show } = useNote();
  const [listening, setListening] = useState(false);
  const toggle = () => {
    if (listening) {
      voice.stopListening().catch(() => undefined);
      return;
    }
    setListening(true);
    voice
      .listen()
      .then(text => {
        if (text.trim()) onHeard(text.trim());
      })
      .catch(e => show(errorText(e), 'problem'))
      .finally(() => setListening(false));
  };
  return (
    <button type="button" className="send send-quiet" aria-label={listening ? 'Stop listening' : 'Ask by speaking'} aria-pressed={listening} disabled={disabled} onClick={toggle}>
      {listening ? <MicOff aria-hidden="true" className="icon" /> : <Mic aria-hidden="true" className="icon" />}
    </button>
  );
}

export function ReadAloud({ answer }: { answer: AnswerState }) {
  const { voice } = useDevice();
  const { show } = useNote();
  const [speaking, setSpeaking] = useState(false);
  if (isLive(answer) || answer.status !== 'completed') return null;
  const toggle = () => {
    if (speaking) {
      voice.stop().catch(() => undefined);
      setSpeaking(false);
      return;
    }
    setSpeaking(true);
    voice
      .speak(spokenAnswer(answer))
      .catch(e => show(errorText(e), 'problem'))
      .finally(() => setSpeaking(false));
  };
  return (
    <button type="button" className="button read-aloud" onClick={toggle}>
      {speaking ? <Square aria-hidden="true" className="icon-small" /> : <Volume2 aria-hidden="true" className="icon-small" />}
      {speaking ? 'Stop reading' : 'Read aloud'}
    </button>
  );
}
