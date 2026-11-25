/**
 * What a transcript's buttons do outside the page: open a web source in the browser, stop a
 * command the answer is running. By default they act in this window (Ask, the in-page chat).
 * The floating chat over the embedded preview (`features/dock`) provides its own, which sends
 * them to the main window: the dock's webview may call no command of its own.
 */
import { createContext, useContext } from 'react';
import { openUrl } from '@tauri-apps/plugin-opener';
import { agentApi } from './agentApi';

export interface TranscriptEffects {
  /** Open `url` (a web source) in the system browser, never inside the app. */
  openUrl: (url: string) => Promise<void>;
  /** Stop the command `commandId`; resolves whether it was still running. */
  stopCommand: (commandId: string) => Promise<boolean>;
}

export const MAIN_TRANSCRIPT_EFFECTS: TranscriptEffects = {
  openUrl: url => openUrl(url),
  stopCommand: commandId => agentApi.stopCommand(commandId),
};

export const TranscriptEffectsContext = createContext<TranscriptEffects>(MAIN_TRANSCRIPT_EFFECTS);

export function useTranscriptEffects(): TranscriptEffects {
  return useContext(TranscriptEffectsContext);
}
