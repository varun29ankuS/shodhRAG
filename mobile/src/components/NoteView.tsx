/** A short note at the top ("Sent to your computer"), gone after a few seconds. */
import { AnimatePresence, motion } from 'framer-motion';
import { Check, TriangleAlert } from 'lucide-react';
import { useNote } from '../lib/context';
import { noteIn } from '../lib/motion';

export function NoteView() {
  const { note } = useNote();
  return (
    <AnimatePresence>
      {note && (
        <motion.div
          key={note.id}
          role="status"
          className={`note${note.tone === 'problem' ? ' note-problem' : ''}`}
          variants={noteIn}
          initial="hidden"
          animate="shown"
          exit="gone"
        >
          {note.tone === 'problem' ? <TriangleAlert aria-hidden="true" className="icon-small" /> : <Check aria-hidden="true" className="icon-small" />}
          <span>{note.text}</span>
        </motion.div>
      )}
    </AnimatePresence>
  );
}
