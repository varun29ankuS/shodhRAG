import { invoke } from '@tauri-apps/api/core';

/** Token counts (always estimates: `method` says how). `code_intel_commands.rs`. */
export interface SavingsTally {
  savedTokens: number;
  originalTokens: number;
  sentTokens: number;
  commands: number;
  shortenedCommands: number;
  estimated: boolean;
  method: string;
}

/** One Code conversation's savings and its output-shortening switch. */
export interface ConversationSavings extends SavingsTally {
  conversationId: string;
  compressionOn: boolean;
}

/** The running total over every Code conversation (Activity). */
export interface TotalSavings extends SavingsTally {
  conversations: number;
  since: string | null;
}

/** A project a bundled Code skill was written from. */
export interface SkillSource {
  project: string;
  url: string;
  commit: string;
  license: string;
  skills: string[];
}

/** One of Shodh's bundled Code skills (Settings → Code). */
export interface CodeSkill {
  name: string;
  title: string;
  summary: string;
  /** When the agent loads it. */
  when: string;
  sources: SkillSource[];
  enabled: boolean;
}

export const codeIntelApi = {
  /** A Code conversation's tokens saved. */
  savings: (conversationId: string) => invoke<ConversationSavings>('code_savings_get', { conversationId }),
  /** Turn output shortening on or off for one conversation (from its next command). */
  setShortening: (conversationId: string, enabled: boolean) =>
    invoke<ConversationSavings>('code_savings_set_enabled', { conversationId, enabled }),
  /** The tokens saved over every Code conversation. */
  total: () => invoke<TotalSavings>('code_savings_total'),
  /** The bundled Code skills with their switches. */
  skills: () => invoke<CodeSkill[]>('code_skills_list'),
  /** Turn a bundled Code skill on or off; returns them all. */
  setSkill: (name: string, enabled: boolean) => invoke<CodeSkill[]>('code_skills_set_enabled', { name, enabled }),
};

/** Tauri command errors arrive as plain strings. */
export function errorText(err: unknown): string {
  if (typeof err === 'string') return err;
  if (err instanceof Error) return err.message;
  return String(err);
}
