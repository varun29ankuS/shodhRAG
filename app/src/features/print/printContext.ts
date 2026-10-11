import { createContext, useContext } from 'react';

/**
 * True inside the print view: visuals render a still, control-free version
 * (a simulation shows its first frame and says it is interactive in Shodh).
 */
export const PrintModeContext = createContext(false);

export function usePrintMode(): boolean {
  return useContext(PrintModeContext);
}
