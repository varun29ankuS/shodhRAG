/**
 * Where diagrams are shown: the Code mode workspace whose code folder their
 * file paths are checked against and opened from (null outside Code mode).
 */

import { createContext, useContext } from 'react';

export interface DiagramHost {
  codeWorkspaceId: string | null;
}

export const DiagramHostContext = createContext<DiagramHost>({ codeWorkspaceId: null });

export function useDiagramHost(): DiagramHost {
  return useContext(DiagramHostContext);
}
