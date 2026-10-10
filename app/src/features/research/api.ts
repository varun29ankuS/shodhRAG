/**
 * Research commands (snippets and Result statements). Errors arrive as
 * `{ code, message }`; `toResearchError` normalises anything thrown. Shapes
 * are in `./types`.
 */

import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import type { UnlistenFn } from '@tauri-apps/api/event';
import { readSnippet } from './snippetModel';
import type {
  Comparison,
  ExtractionReport,
  NewSnippetInput,
  PaperResults,
  ResultFacets,
  ResultFilter,
  Snippet,
  SnippetListQuery,
  SnippetPatch,
  SnippetTable,
  VisionCapability,
} from './types';
import { RESEARCH_CHANGED_EVENT } from './types';

export type ResearchErrorCode = 'not_found' | 'invalid' | 'unavailable' | 'storage' | 'conflict' | 'unknown';

export interface ResearchFailure {
  code: ResearchErrorCode;
  message: string;
}

const CODES: readonly ResearchErrorCode[] = ['not_found', 'invalid', 'unavailable', 'storage', 'conflict'];

export function toResearchError(error: unknown): ResearchFailure {
  if (typeof error === 'object' && error !== null) {
    const e = error as { code?: unknown; message?: unknown };
    if (typeof e.message === 'string') {
      const code = typeof e.code === 'string' && (CODES as readonly string[]).includes(e.code) ? (e.code as ResearchErrorCode) : 'unknown';
      return { code, message: e.message };
    }
  }
  if (typeof error === 'string') return { code: 'unknown', message: error };
  return { code: 'unknown', message: error instanceof Error ? error.message : 'The research store could not be reached.' };
}

function snippetOrThrow(value: unknown): Snippet {
  const snippet = readSnippet(value);
  if (!snippet) throw { code: 'storage', message: 'The snippet the app returned is unreadable.' } satisfies ResearchFailure;
  return snippet;
}

export const researchApi = {
  createSnippet: async (input: NewSnippetInput) => snippetOrThrow(await invoke<unknown>('snippets_create', { input })),
  listSnippets: async (query: SnippetListQuery) => {
    const list = await invoke<unknown[]>('snippets_list', { query });
    return (Array.isArray(list) ? list : []).map(readSnippet).filter((s): s is Snippet => s !== null);
  },
  getSnippet: async (id: string) => snippetOrThrow(await invoke<unknown>('snippets_get', { id })),
  /** Base64 PNG, or null when the image was never stored (agent-made snippets). */
  snippetImage: (id: string) => invoke<string | null>('snippets_image', { id }),
  setSnippetImage: async (id: string, imagePng: string) => snippetOrThrow(await invoke<unknown>('snippets_set_image', { id, imagePng })),
  updateSnippet: async (id: string, patch: SnippetPatch) => snippetOrThrow(await invoke<unknown>('snippets_update', { id, patch })),
  deleteSnippet: (id: string) => invoke<void>('snippets_delete', { id }),
  snippetTable: (id: string) => invoke<SnippetTable | null>('snippets_table', { id }),
  transcribeSnippet: async (id: string) => snippetOrThrow(await invoke<unknown>('snippets_transcribe_latex', { id })),
  visionCapability: () => invoke<VisionCapability>('vision_capability'),
  extractResults: (filePath: string, workspace: string | null, useModel: boolean) =>
    invoke<ExtractionReport>('results_extract', { filePath, workspace, useModel }),
  listResults: (filePath: string) => invoke<PaperResults>('results_list', { filePath }),
  reviewResult: (id: string, accept: boolean) => invoke<void>('results_review', { id, accept }),
  queryResults: (filter: ResultFilter) => invoke<Comparison>('results_query', { filter }),
  resultFacets: (workspace: string | null) => invoke<ResultFacets>('results_facets', { workspace }),
};

export interface ResearchChange {
  kind: 'snippet' | 'result' | 'graph' | null;
  filePath: string | null;
}

/** `research-changed`: snippets, results or the citation graph (of a file, when known) changed, including the agent's writes. */
export function onResearchChanged(handler: (change: ResearchChange) => void): Promise<UnlistenFn> {
  return listen<{ kind?: unknown; filePath?: unknown }>(RESEARCH_CHANGED_EVENT, event => {
    const kind = event.payload?.kind;
    const filePath = event.payload?.filePath;
    handler({
      kind: kind === 'snippet' || kind === 'result' || kind === 'graph' ? kind : null,
      filePath: typeof filePath === 'string' ? filePath : null,
    });
  });
}

/** Base64 of bytes (chunked: large images exceed the argument limit of `fromCharCode`). */
export function bytesToBase64(bytes: Uint8Array): string {
  let binary = '';
  const chunk = 0x8000;
  for (let i = 0; i < bytes.length; i += chunk) binary += String.fromCharCode(...bytes.subarray(i, i + chunk));
  return btoa(binary);
}

/** Bytes of base64 (no data: prefix). */
export function base64ToBytes(base64: string): Uint8Array {
  const binary = atob(base64);
  const out = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) out[i] = binary.charCodeAt(i);
  return out;
}
