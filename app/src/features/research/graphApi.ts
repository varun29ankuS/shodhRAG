/**
 * Citation graph commands. Errors arrive as `{ code, message }` like the other
 * research commands (see `toResearchError`).
 */

import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import type { UnlistenFn } from '@tauri-apps/api/event';
import type { BuildProgress, BuildReport, ConceptView, GraphStatus, GraphViewData, PaperDetail, PaperNode } from './graphTypes';

export const GRAPH_PROGRESS_EVENT = 'citation-graph-progress';

export interface PaperFilterInput {
  method?: string | null;
  dataset?: string | null;
  author?: string | null;
  yearFrom?: number | null;
  yearTo?: number | null;
  inLibraryOnly?: boolean;
}

export const graphApi = {
  status: () => invoke<GraphStatus>('paper_graph_status'),
  /** `online`: look papers up on OpenAlex when the privacy policy allows it. */
  build: (online: boolean) => invoke<BuildReport>('paper_graph_build', { online }),
  view: () => invoke<GraphViewData>('paper_graph_view'),
  paper: (paper: string) => invoke<PaperDetail>('paper_get', { paper }),
  find: (filter: PaperFilterInput) => invoke<PaperNode[]>('papers_find', { filter }),
  concept: (kind: 'method' | 'dataset', id: string) => invoke<ConceptView>('paper_concept', { concept: { kind, id } }),
};

export function onGraphProgress(handler: (progress: BuildProgress) => void): Promise<UnlistenFn> {
  return listen<BuildProgress>(GRAPH_PROGRESS_EVENT, event => handler(event.payload));
}
