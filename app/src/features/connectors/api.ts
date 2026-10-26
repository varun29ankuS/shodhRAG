/**
 * Tauri commands of the Slack card (`connectors/slack.rs`). Every call is the user's own
 * action; the tokens go in once (`slack_connect`) and never come back.
 */
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import {
  SLACK_STATUS_EVENT,
  type ChannelChoice,
  type Person,
  type SlackMemoryView,
  type SlackStatus,
} from './slackModel';

export const slackApi = {
  status: () => invoke<SlackStatus>('slack_status'),
  connect: (appToken: string, botToken: string) => invoke<SlackStatus>('slack_connect', { appToken, botToken }),
  setEnabled: (enabled: boolean) => invoke<SlackStatus>('slack_set_enabled', { enabled }),
  setAnswerWorkspace: (workspaceId: string | null) =>
    invoke<SlackStatus>('slack_set_answer_workspace', { workspaceId }),
  listChannels: () => invoke<ChannelChoice[]>('slack_list_channels'),
  setAnswerChannels: (channelIds: string[]) => invoke<SlackStatus>('slack_set_answer_channels', { channelIds }),
  setDailyCap: (dailyCap: number) => invoke<SlackStatus>('slack_set_daily_cap', { dailyCap }),
  /** `forget`: also forget everything indexed and every memory saved from Slack. */
  disconnect: (forget: boolean) => invoke<SlackStatus>('slack_disconnect', { forget }),
  indexChannel: (channelId: string, workspaceId: string, confirmShared: boolean) =>
    invoke<SlackStatus>('slack_index_channel', { channelId, workspaceId, confirmShared }),
  forgetChannel: (channelId: string) => invoke<SlackStatus>('slack_forget_channel', { channelId }),
  setIndexWindow: (days: number) => invoke<SlackStatus>('slack_set_index_window', { days }),
  listPeople: () => invoke<Person[]>('slack_list_people'),
  setOwner: (userId: string | null) => invoke<SlackStatus>('slack_set_owner', { userId }),
  memories: () => invoke<SlackMemoryView[]>('slack_memories'),
  promoteMemory: (id: string) => invoke<SlackMemoryView[]>('slack_promote_memory', { id }),
  deleteMemory: (id: string) => invoke<SlackMemoryView[]>('slack_delete_memory', { id }),
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
