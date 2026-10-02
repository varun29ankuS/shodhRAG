import { useCallback, useEffect, useRef, useState } from 'react';
import { notify } from '../../lib/notify';

/** Minimal typing for the Web Speech API (not in TypeScript's DOM lib). */
interface SpeechRecognitionAlternativeLike {
  transcript: string;
}
interface SpeechRecognitionResultLike {
  isFinal: boolean;
  0: SpeechRecognitionAlternativeLike;
}
interface SpeechRecognitionEventLike {
  results: ArrayLike<SpeechRecognitionResultLike>;
}
interface SpeechRecognitionErrorEventLike {
  error: string;
}
interface SpeechRecognitionLike {
  continuous: boolean;
  interimResults: boolean;
  lang: string;
  onresult: ((event: SpeechRecognitionEventLike) => void) | null;
  onend: (() => void) | null;
  onerror: ((event: SpeechRecognitionErrorEventLike) => void) | null;
  start: () => void;
  stop: () => void;
}
type SpeechRecognitionCtor = new () => SpeechRecognitionLike;

function getRecognitionCtor(): SpeechRecognitionCtor | null {
  if (typeof window === 'undefined') return null;
  const w = window as unknown as {
    SpeechRecognition?: SpeechRecognitionCtor;
    webkitSpeechRecognition?: SpeechRecognitionCtor;
  };
  return w.SpeechRecognition ?? w.webkitSpeechRecognition ?? null;
}

/**
 * Dictation into the composer. Results replace the text typed before
 * dictation started plus the current transcript, so interim results never
 * accumulate.
 */
export function useVoiceInput(getText: () => string, setText: (text: string) => void) {
  const [listening, setListening] = useState(false);
  const recognitionRef = useRef<SpeechRecognitionLike | null>(null);
  const supported = getRecognitionCtor() !== null;

  useEffect(() => () => recognitionRef.current?.stop(), []);

  const toggle = useCallback(() => {
    if (recognitionRef.current) {
      recognitionRef.current.stop();
      return;
    }
    const Ctor = getRecognitionCtor();
    if (!Ctor) {
      notify.warning('Voice input is not supported on this system');
      return;
    }
    const recognition = new Ctor();
    recognition.continuous = false;
    recognition.interimResults = true;
    recognition.lang = navigator.language || 'en-US';

    const before = getText();
    const base = before + (before && !before.endsWith(' ') ? ' ' : '');

    recognition.onresult = event => {
      let finalText = '';
      let interim = '';
      for (let i = 0; i < event.results.length; i++) {
        const result = event.results[i];
        if (result.isFinal) finalText += result[0].transcript;
        else interim += result[0].transcript;
      }
      setText(base + (finalText || interim));
    };
    recognition.onend = () => {
      recognitionRef.current = null;
      setListening(false);
    };
    recognition.onerror = event => {
      recognitionRef.current = null;
      setListening(false);
      if (event.error !== 'aborted' && event.error !== 'no-speech') {
        notify.error(`Voice input failed: ${event.error}`);
      }
    };

    recognitionRef.current = recognition;
    recognition.start();
    setListening(true);
  }, [getText, setText]);

  return { supported, listening, toggle };
}
