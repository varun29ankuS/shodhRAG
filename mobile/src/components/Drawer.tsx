/**
 * The menu: every section of the desktop, in the desktop's order and with
 * its names, plus Approvals; the connection; and Appearance (Phone, Light,
 * Dark). It slides in from the left and closes with a swipe back to the
 * left, a tap beside it, or Back.
 */
import { motion, type PanInfo } from 'framer-motion';
import { Activity, BookOpen, ListChecks, MessageCircle, Layers, Plug, Settings, Shapes, ShieldCheck, type LucideIcon } from 'lucide-react';
import { useLink, useNav, useThemeChoice } from '../lib/context';
import { fade, glide } from '../lib/motion';
import { SECTIONS, SECTION_LABELS, type Section } from '../lib/nav';
import { THEME_CHOICES } from '../lib/theme';
import { connectionWords, Dot } from './Connection';
import logo from '../../src-tauri/icons/icon.png';

export const SECTION_ICONS: Record<Section, LucideIcon> = {
  ask: MessageCircle,
  artifacts: Shapes,
  library: BookOpen,
  workspaces: Layers,
  connectors: Plug,
  tasks: ListChecks,
  approvals: ShieldCheck,
  activity: Activity,
  settings: Settings,
};

/** Let go this far left (px), or this fast (px/s), and the menu closes. */
const CLOSE_DISTANCE = 80;
const CLOSE_SPEED = 500;

export function Drawer({ waiting }: { waiting: number }) {
  const { nav, dispatch, back } = useNav();
  const { status } = useLink();
  const { choice, choose } = useThemeChoice();
  const released = (_: unknown, info: PanInfo) => {
    if (info.offset.x < -CLOSE_DISTANCE || info.velocity.x < -CLOSE_SPEED) back();
  };
  return (
    <div className="menu-layer">
      <motion.button type="button" className="scrim" aria-label="Close menu" onClick={back} variants={fade} initial="hidden" animate="shown" exit="gone" />
      <motion.nav
        className="drawer"
        aria-label="All sections"
        initial={{ x: -320 }}
        animate={{ x: 0 }}
        exit={{ x: -320 }}
        transition={glide}
        drag="x"
        dragConstraints={{ left: 0, right: 0 }}
        dragElastic={{ left: 0.8, right: 0.04 }}
        onDragEnd={released}
      >
        <div className="drawer-head">
          <img src={logo} alt="" className="brand-logo" width={40} height={40} />
          <span className="row-text">
            <span className="drawer-name serif">Shodh</span>
            <span className="drawer-line connection">
              <Dot status={status} small />
              {connectionWords(status).short}
            </span>
          </span>
        </div>
        {SECTIONS.map(section => {
          const Icon = SECTION_ICONS[section];
          const count = section === 'approvals' ? waiting : 0;
          return (
            <button
              key={section}
              type="button"
              className="menu-item"
              aria-current={nav.section === section ? 'page' : undefined}
              onClick={() => dispatch({ type: 'go', section })}
            >
              <Icon aria-hidden="true" className="icon" />
              <span className="menu-label">{SECTION_LABELS[section]}</span>
              {count > 0 && (
                <span className="count" aria-label={`${count} waiting`}>
                  {count}
                </span>
              )}
            </button>
          );
        })}
        <div className="appearance" role="radiogroup" aria-label="Appearance">
          <span className="appearance-label">Appearance</span>
          <div className="segmented">
            {THEME_CHOICES.map(option => (
              <button
                key={option.value}
                type="button"
                role="radio"
                className="segment"
                aria-checked={choice === option.value}
                onClick={() => choose(option.value)}
              >
                {option.label}
              </button>
            ))}
          </div>
        </div>
      </motion.nav>
    </div>
  );
}
