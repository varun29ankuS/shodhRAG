/**
 * A bottom sheet: it springs up, follows the finger when dragged down, and
 * closes when let go far enough or fast enough (or snaps back). The page
 * behind is dimmed; tapping it closes the sheet.
 */
import type { ReactNode } from 'react';
import { motion, type PanInfo } from 'framer-motion';
import { fade, glide } from '../lib/motion';

/** Let go this far down (px), or this fast (px/s), and the sheet closes. */
const CLOSE_DISTANCE = 120;
const CLOSE_SPEED = 600;

export function Sheet({ label, onClose, children }: { label: string; onClose: () => void; children: ReactNode }) {
  const released = (_: unknown, info: PanInfo) => {
    if (info.offset.y > CLOSE_DISTANCE || info.velocity.y > CLOSE_SPEED) onClose();
  };
  return (
    <div className="overlay">
      <motion.button
        type="button"
        className="scrim"
        aria-label="Close"
        onClick={onClose}
        variants={fade}
        initial="hidden"
        animate="shown"
        exit="gone"
      />
      <motion.section
        className="sheet"
        role="dialog"
        aria-modal="true"
        aria-label={label}
        initial={{ y: 600 }}
        animate={{ y: 0 }}
        exit={{ y: 600 }}
        transition={glide}
        drag="y"
        dragConstraints={{ top: 0, bottom: 0 }}
        dragElastic={{ top: 0.05, bottom: 0.8 }}
        onDragEnd={released}
      >
        <span className="grabber" aria-hidden="true" />
        {children}
      </motion.section>
    </div>
  );
}
