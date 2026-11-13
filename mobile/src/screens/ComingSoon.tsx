/**
 * A section of the desktop the phone does not show yet: its name, what it is
 * for in the desktop's own words, and where to find it now. No data.
 */
import { motion } from 'framer-motion';
import { Monitor } from 'lucide-react';
import { TopBar } from '../components/Bars';
import { SECTION_ICONS } from '../components/Drawer';
import { viewIn } from '../lib/motion';
import { SECTION_LABELS } from '../lib/nav';
import { VIEW_TAB_DESCRIPTIONS, type ViewTab } from '../../../app/src/lib/viewTabs.ts';

export function ComingSoon({ section }: { section: ViewTab }) {
  const Icon = SECTION_ICONS[section];
  const label = SECTION_LABELS[section];
  return (
    <motion.div className="view" variants={viewIn} initial="hidden" animate="shown" exit="gone">
      <div className="scroll">
        <TopBar brand={false} />
        <h1 className="title serif pad">{label}</h1>
        <section className="panel">
          <span className="panel-icon">
            <Icon aria-hidden="true" className="icon" />
          </span>
          <p className="panel-title">Coming to the phone soon</p>
          <p className="panel-text">{`${VIEW_TAB_DESCRIPTIONS[section]}.`}</p>
          <p className="on-computer">
            <Monitor aria-hidden="true" className="icon" />
            <span>{`Open ${label} in Shodh on your computer.`}</span>
          </p>
        </section>
      </div>
    </motion.div>
  );
}
