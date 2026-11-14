/** The top of a screen: the menu button, the elephant, the connection; or Back. */
import type { ReactNode } from 'react';
import { ChevronLeft, Menu } from 'lucide-react';
import { useLink, useNav } from '../lib/context';
import { connectionWords, Dot } from './Connection';
import logo from '../../src-tauri/icons/icon.png';

export function MenuButton() {
  const { dispatch } = useNav();
  return (
    <button type="button" className="icon-button" aria-label="Open menu" onClick={() => dispatch({ type: 'drawer', open: true })}>
      <Menu aria-hidden="true" className="icon" />
    </button>
  );
}

/** The top of a section: the menu, the elephant and the connection. */
export function TopBar({ brand = true }: { brand?: boolean }) {
  const { status } = useLink();
  const { dispatch } = useNav();
  const words = connectionWords(status);
  return (
    <header className="topbar">
      <span className="brand">
        <MenuButton />
        {brand && (
          <>
            <img src={logo} alt="" className="brand-logo" width={36} height={36} />
            <span className="brand-name serif">Shodh</span>
          </>
        )}
      </span>
      <button
        type="button"
        className="icon-button"
        aria-label={`${words.short}. Open settings`}
        onClick={() => dispatch({ type: 'go', section: 'settings' })}
      >
        <Dot status={status} />
      </button>
    </header>
  );
}

/** The top of a pushed screen: Back, and what is on the right. */
export function BackBar({ label, children }: { label: string; children?: ReactNode }) {
  const { back } = useNav();
  return (
    <header className="topbar">
      <button type="button" className="back" onClick={back}>
        <ChevronLeft aria-hidden="true" className="icon" />
        <span>{label}</span>
      </button>
      {children}
    </header>
  );
}
