/**
 * Tauri commands of the Telegram page (`connectors/telegram.rs`). Every call is the user's
 * own action; the token goes in once (`telegram_connect`) and never comes back.
 */
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { TELEGRAM_STATUS_EVENT, type LinkInvite, type TelegramStatus } from './telegramModel';

export const telegramApi = {
  status: () => invoke<TelegramStatus>('telegram_status'),
  connect: (token: string) => invoke<TelegramStatus>('telegram_connect', { token }),
  /** A one-time link (and QR code) for the user's own account, or an invite. */
  linkCode: (kind: 'owner' | 'invite') => invoke<LinkInvite>('telegram_link_code', { kind }),
  unlinkOwner: () => invoke<TelegramStatus>('telegram_unlink_owner'),
  setEnabled: (enabled: boolean) => invoke<TelegramStatus>('telegram_set_enabled', { enabled }),
  setAnswerWorkspace: (workspaceId: string | null) =>
    invoke<TelegramStatus>('telegram_set_answer_workspace', { workspaceId }),
  setGroup: (groupId: string, allowed: boolean) => invoke<TelegramStatus>('telegram_set_group', { groupId, allowed }),
  removePerson: (personId: string) => invoke<TelegramStatus>('telegram_remove_person', { personId }),
  setDailyCap: (dailyCap: number) => invoke<TelegramStatus>('telegram_set_daily_cap', { dailyCap }),
  /** `forget`: also remove every Telegram conversation from the app. */
  disconnect: (forget: boolean) => invoke<TelegramStatus>('telegram_disconnect', { forget }),
};

/** Calls `handler` with every status the backend emits; returns the unsubscribe. */
export function onTelegramStatus(handler: (status: TelegramStatus) => void): () => void {
  let disposed = false;
  let unlisten: (() => void) | null = null;
  listen<TelegramStatus>(TELEGRAM_STATUS_EVENT, event => handler(event.payload))
    .then(fn => {
      if (disposed) fn();
      else unlisten = fn;
    })
    .catch(err => console.error(`Failed to listen for ${TELEGRAM_STATUS_EVENT}:`, err));
  return () => {
    disposed = true;
    if (unlisten) unlisten();
  };
}
