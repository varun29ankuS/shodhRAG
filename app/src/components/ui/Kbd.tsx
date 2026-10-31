import React from 'react';
import { cn } from '../../lib/utils';
import { IS_MAC_PLATFORM, shortcutText } from '../../lib/shortcuts';
import type { ShortcutId } from '../../lib/shortcuts';

/** Whether this computer is a Mac, read once. */
export const IS_MAC = IS_MAC_PLATFORM;

interface KbdProps {
  shortcut: ShortcutId;
  className?: string;
  /** On a filled (accent) button: the hint takes the button's text colour. */
  onAccent?: boolean;
}

/**
 * A shortcut shown on its button at all times ("Ctrl+N", "⌘N"), written from the
 * shortcut registry (`lib/shortcuts.ts`) so it always matches the binding. Hidden
 * from screen readers: the button names it with `aria-keyshortcuts`.
 */
export function Kbd({ shortcut, className, onAccent = false }: KbdProps) {
  return (
    <kbd
      aria-hidden="true"
      className={cn(
        'inline-flex items-center shrink-0 font-mono text-caption font-normal leading-none whitespace-nowrap',
        onAccent ? 'opacity-80' : 'text-shodh-text-muted',
        className,
      )}
    >
      {shortcutText(shortcut, IS_MAC)}
    </kbd>
  );
}

export default Kbd;
