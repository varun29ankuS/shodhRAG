import { useState, useEffect, useCallback } from 'react';
import { matchesShortcut } from '../lib/shortcuts';

export interface UseCommandPaletteReturn {
  open: boolean;
  openPalette: () => void;
  closePalette: () => void;
}

export function useCommandPalette(): UseCommandPaletteReturn {
  const [open, setOpen] = useState(false);

  const openPalette = useCallback(() => setOpen(true), []);
  const closePalette = useCallback(() => setOpen(false), []);

  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if (matchesShortcut(e, 'commandPalette')) {
        e.preventDefault();
        setOpen(prev => !prev);
      }
    };
    window.addEventListener('keydown', handleKeyDown);
    return () => window.removeEventListener('keydown', handleKeyDown);
  }, []);

  return { open, openPalette, closePalette };
}
