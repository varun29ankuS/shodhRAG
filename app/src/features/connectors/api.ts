/**
 * Tauri commands of the Slack card (`connectors/slack.rs`). Every call is the user's own
 * action; the tokens go in once (`slack_connect`) and never come back.
 */
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { SLACK_STATUS_EVENT, type ChannelChoice, type SlackStatus } from './slackModel';

export const slackApi = {
  status: () => invoke<SlackStatus>('slack_status'),
  connect: (appToken: string, botToken: string) => invoke<SlackStatus>('slack_connect', { appToken, botToken }),
  setEnabled: (enabled: boolean) => invoke<SlackStatus>('slack_set_enabled', { enabled }),
  setAnswerWorkspace: (workspaceId: string | null) =>
    invoke<SlackStatus>('slack_set_answer_workspace', { workspaceId }),
  listChannels: () => invoke<ChannelChoice[]>('slack_list_channels'),
  setAnswerChannels: (channelIds: string[]) => invoke<SlackStatus>('slack_set_answer_channels', { channelIds }),
  setDailyCap: (dailyCap: number) => invoke<SlackStatus>('slack_set_daily_cap', { dailyCap }),
  disconnect: () => invoke<SlackStatus>('slack_disconnect'),
};

/** The command's error as text (the commands reject with a plain message). */
export function slackError(err: unknown): string {
  if (typeof err === 'string') return err;
  if (err instanceof Error) return err.message;
  return String(err);
}

/** Calls `handler` with every status the backend emits; returns the unsubscribe. */
export function onSlackStatus(handler: (status: SlackStatus) => void): () => void {
  let disposed = false;
  let unlisten: (() => void) | null = null;
  listen<SlackStatus>(SLACK_STATUS_EVENT, event => handler(event.payload))
    .then(fn => {
      if (disposed) fn();
      else unlisten = fn;
    })
    .catch(err => console.error(`Failed to listen for ${SLACK_STATUS_EVENT}:`, err));
  return () => {
    disposed = true;
    if (unlisten) unlisten();
  };
}
