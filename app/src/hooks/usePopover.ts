import { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react';
import { cn } from '../lib/utils';

/** Portal popover surface (quick menus, the New menu). */
export const POPOVER = 'fixed z-50 min-w-[180px] rounded-xl border border-shodh-border-strong bg-shodh-surface p-1 shadow-[0_12px_36px_rgba(0,0,0,0.32)] shell-pop';

export const MENU_ITEM = cn(
  'w-full h-8 px-2.5 rounded-lg inline-flex items-center gap-2 text-left text-[12.5px] text-shodh-text hover:bg-shodh-raised focus:bg-shodh-raised focus:outline-none',
);

/**
 * Popover anchored under (or above, near the bottom) a trigger, rendered in a
 * portal so scrolling lists cannot clip it. Closes on outside press and Esc,
 * returning focus to the trigger.
 */
export function usePopover() {
  const [open, setOpen] = useState(false);
  const [pos, setPos] = useState<{ left: number; top: number } | null>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const popRef = useRef<HTMLDivElement>(null);

  const close = useCallback((refocus: boolean) => {
    setOpen(false);
    if (refocus) triggerRef.current?.focus();
  }, []);

  useLayoutEffect(() => {
    if (!open || !triggerRef.current) return;
    const anchor = triggerRef.current.getBoundingClientRect();
    const height = popRef.current?.offsetHeight ?? 0;
    const width = popRef.current?.offsetWidth ?? 0;
    const below = anchor.bottom + 4;
    const top = below + height > window.innerHeight - 8 ? Math.max(8, anchor.top - height - 4) : below;
    const left = Math.min(Math.max(8, anchor.left), window.innerWidth - width - 8);
    setPos({ left, top });
  }, [open]);

  useEffect(() => {
    if (!open) return;
    const onPointer = (e: PointerEvent) => {
      const target = e.target as Node;
      if (popRef.current?.contains(target) || triggerRef.current?.contains(target)) return;
      close(false);
    };
    window.addEventListener('pointerdown', onPointer, true);
    return () => window.removeEventListener('pointerdown', onPointer, true);
  }, [open, close]);

  return { open, setOpen, close, pos, triggerRef, popRef };
}

