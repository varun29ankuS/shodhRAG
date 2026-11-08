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
  /** Conversations ever counted, deleted ones included. */
  conversations: number;
  since: string | null;
  /** Shortening in Code conversations that did not choose for themselves. */
  defaultOn: boolean;
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
  /** Shortening for Code conversations that did not choose for themselves (Settings → Code). */
  setDefaultShortening: (enabled: boolean) => invoke<TotalSavings>('code_savings_set_default', { enabled }),
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

/** What a project's code map holds (`shodh_rag::code_map::MapStatus`). */
export interface CodeMapCounts {
  files: number;
  symbols: number;
  references: number;
  /** Source files by language: [language, files]. */
  languages: [string, number][];
  /** When a refresh last went through every file. */
  completeAt: string | null;
  updatedAt: string | null;
  /** When a refresh last ended. */
  checkedAt?: string | null;
  /** Files the last listing found. */
  listed?: number | null;
  partial: boolean;
}

/** What the last refresh did. */
export interface CodeMapRefresh {
  listed: number;
  read: number;
  unchanged: number;
  removed: number;
  skipped: number;
  millis: number;
  stopped: boolean;
  partial: boolean;
}

/** A Code project's code map as the UI shows it (`code_map_commands::CodeMapView`). */
export interface CodeMapView {
  workspaceId: string | null;
  sourceId: string;
  name: string;
  state: 'idle' | 'queued' | 'building' | 'waiting' | 'failed';
  done?: number;
  /** Why a refresh waits: indexing is paused, or other indexing runs first. */
  reason?: 'paused' | 'otherIndexing';
  total?: number;
  error?: string;
  /** Indexing is paused: a refresh waits. */
  paused: boolean;
  watching: boolean;
  map: CodeMapCounts | null;
  lastRun: CodeMapRefresh | null;
  /** The hold `open` took on the map; `close` gives it back. */
  lease?: number | null;
}

/** A definition the code map knows (`shodh_rag::code_map::SymbolHit`). */
export interface SymbolHit {
  name: string;
  kind: string;
  path: string;
  line: number;
  endLine: number;
  container: string | null;
  module: string;
  signature: string;
  exported: boolean;
  language: string;
}

export interface OutlineFile {
  path: string;
  module: string;
  language: string;
  lines: number;
  symbols: SymbolHit[];
  skipped: string | null;
}

export interface Outline {
  target: string;
  files: OutlineFile[];
  truncated: boolean;
}

/** The code map's commands (`code_map_commands.rs`). */
export const codeMapApi = {
  /** A Code conversation of the workspace is shown: build or refresh its map and watch its folder. */
  open: (workspaceId: string) => invoke<CodeMapView>('code_map_open', { workspaceId }),
  /** It is no longer shown. */
  close: (lease: number) => invoke<boolean>('code_map_close', { lease }),
  status: (workspaceId: string) => invoke<CodeMapView>('code_map_status', { workspaceId }),
  /** Empty the map and read every file again. */
  rebuild: (workspaceId: string) => invoke<CodeMapView>('code_map_rebuild', { workspaceId }),
  /** Stop the refresh running now. */
  cancel: (workspaceId: string) => invoke<CodeMapView>('code_map_cancel', { workspaceId }),
  /** A file's symbols. */
  outline: (workspaceId: string, path: string) => invoke<Outline>('code_map_outline', { workspaceId, path }),
  /** Symbols whose names start with `query`. */
  find: (workspaceId: string, query: string) => invoke<SymbolHit[]>('code_map_find', { workspaceId, query }),
};
