/**
 * Which view the shell shows for the view asked for. A view that can render
 * at once (its code loaded) is shown in the same commit as the click; one
 * whose code is still loading is shown through the deferred value, which
 * keeps the current view on screen until it can render, so a switch never
 * shows an empty pane or a loading placeholder. Not always deferred: while
 * an answer streams the shell re-renders every frame and would keep
 * restarting a deferred render of a heavy view.
 *
 * No imports: unit-tested under Node (`app/tests/viewSwitch.test.ts`).
 */
export function shownView<V extends string>(requested: V, deferred: V, isReady: (view: V) => boolean): V {
  return isReady(requested) ? requested : deferred;
}
