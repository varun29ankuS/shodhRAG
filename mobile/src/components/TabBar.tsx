/**
 * The floating tab bar: Ask, Library and Approvals, the three used most.
 * The highlight slides to the chosen tab on a spring.
 */
import { motion } from 'framer-motion';
import { useNav } from '../lib/context';
import { snappy } from '../lib/motion';
import { SECTION_LABELS, TABS } from '../lib/nav';
import { SECTION_ICONS } from './Drawer';

export function TabBar({ waiting }: { waiting: number }) {
  const { nav, dispatch } = useNav();
  const index = TABS.indexOf(nav.section);
  // The highlight is one tab wide: it moves by whole widths.
  const x = `${index * 100}%`;
  return (
    <nav className="tabbar" aria-label="Main">
      {index >= 0 && (
        <motion.span className="tab-indicator" aria-hidden="true" initial={false} animate={{ x }} transition={snappy} />
      )}
      {TABS.map(section => {
        const Icon = SECTION_ICONS[section];
        const count = section === 'approvals' ? waiting : 0;
        return (
          <button
            key={section}
            type="button"
            className="tab"
            aria-current={nav.section === section ? 'page' : undefined}
            onClick={() => dispatch({ type: 'go', section })}
          >
            <Icon aria-hidden="true" className="icon-small" />
            <span>{SECTION_LABELS[section]}</span>
            {count > 0 && (
              <span className="tab-count" aria-label={`${count} waiting`}>
                {count}
              </span>
            )}
          </button>
        );
      })}
    </nav>
  );
}
