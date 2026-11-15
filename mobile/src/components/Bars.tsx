/**
 * The top of a screen: the menu button and the connection, over a section's
 * large title; or, on a pushed screen, Back, the screen's title in the
 * middle, and what is on the right.
 */
import type { ReactNode } from 'react';
import { ChevronLeft, Menu } from 'lucide-react';
import { useLink, useNav } from '../lib/context';
import { connectionWords, Dot } from './Connection';

export function MenuButton() {
  const { dispatch } = useNav();
  return (
    <button type="button" className="icon-button" aria-label="Open menu" onClick={() => dispatch({ type: 'drawer', open: true })}>
      <Menu aria-hidden="true" className="icon" />
    </button>
  );
}

/** The top of a section: the menu, and the connection in words (a tap opens Settings). */
export function TopBar() {
  const { status } = useLink();
  const { dispatch } = useNav();
  const words = connectionWords(status);
  return (
    <header className="topbar">
      <MenuButton />
      <button
        type="button"
        className="connection-button"
        aria-label={`${words.short}. Open settings`}
        onClick={() => dispatch({ type: 'go', section: 'settings' })}
      >
        <Dot status={status} small />
        <span aria-hidden="true">{words.short}</span>
      </button>
    </header>
  );
}

/** The top of a pushed screen: Back, the screen's title in the middle, and what is on the right. */
export function BackBar({ label, title = null, children }: { label: string; title?: string | null; children?: ReactNode }) {
  const { back } = useNav();
  return (
    <header className="topbar navbar">
      <span className="navbar-side">
        <button type="button" className="back" onClick={back}>
          <ChevronLeft aria-hidden="true" className="icon-back" />
          <span>{label}</span>
        </button>
      </span>
      {title ? <span className="navbar-title">{title}</span> : <span aria-hidden="true" />}
      <span className="navbar-side navbar-end">{children}</span>
    </header>
  );
}
