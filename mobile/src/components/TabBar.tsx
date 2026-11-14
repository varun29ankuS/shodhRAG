/**
 * The tab bar: Ask, Library and Approvals, the three used most. Full width
 * along the bottom, translucent over what scrolls under it; each tab is its
 * icon over its name, and only the tab shown is in the tint.
 */
import { useNav } from '../lib/context';
import { SECTION_LABELS, TABS } from '../lib/nav';
import { SECTION_ICONS } from './Drawer';

export function TabBar({ waiting }: { waiting: number }) {
  const { nav, dispatch } = useNav();
  return (
    <nav className="tabbar" aria-label="Main">
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
            <Icon aria-hidden="true" className="tab-icon" />
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
