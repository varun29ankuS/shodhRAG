import { useEffect, useRef } from 'react';
import type { NavigationTarget } from './events';
import { peekTarget, subscribeTargets, takeTarget } from './navigation';
import type { TargetKind } from './navigation';

type TargetOf<K extends TargetKind> = Extract<NavigationTarget, { kind: K }>;

/**
 * Run `handler` for agent navigation targets of `kind`: the one pending
 * when the component mounts, and every later one. With `consume: false`
 * the target stays pending for a component deeper in the tree (e.g. a
 * shell that only reveals the view which then applies the target).
 */
export function useNavigationTarget<K extends TargetKind>(
  kind: K,
  handler: (target: TargetOf<K>) => void,
  options: { consume?: boolean } = {},
): void {
  const consume = options.consume ?? true;
  const handlerRef = useRef(handler);
  handlerRef.current = handler;
  useEffect(() => {
    const initial = consume ? takeTarget(kind) : peekTarget(kind);
    if (initial) handlerRef.current(initial);
    return subscribeTargets(target => {
      if (target.kind !== kind) return;
      if (consume) takeTarget(kind);
      handlerRef.current(target as TargetOf<K>);
    });
  }, [kind, consume]);
}
