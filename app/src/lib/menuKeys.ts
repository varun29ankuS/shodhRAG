/**
 * The keys that move through a menu's items (the composer's model menu, in Ask and in the
 * floating chat): Down and Up step and wrap, Home and End jump. One function, so every
 * menu moves the same way; `app/tests/keyboardPath.test.ts` walks it.
 *
 * No imports: the Node tests load it.
 */

/**
 * The item `key` moves the focus to, from item `at` of `count` (`at` is -1 when the focus
 * is on none of them). Null for a key that does not move, and for an empty menu.
 */
export function nextMenuIndex(key: string, at: number, count: number): number | null {
  if (count <= 0) return null;
  switch (key) {
    case 'Home':
      return 0;
    case 'End':
      return count - 1;
    case 'ArrowDown':
      return (at + 1) % count;
    case 'ArrowUp':
      // From no item, Up goes to the last one.
      return at <= 0 ? count - 1 : at - 1;
    default:
      return null;
  }
}
