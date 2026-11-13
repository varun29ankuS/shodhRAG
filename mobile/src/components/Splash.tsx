/**
 * The elephant while the app reads its key and reaches for the computer. It
 * goes as soon as the app knows where it stands; it never holds the app up.
 */
import { motion } from 'framer-motion';
import { bouncy, fade } from '../lib/motion';
import logo from '../../src-tauri/icons/icon.png';

export function Splash() {
  return (
    <motion.div className="splash" role="status" aria-label="Shodh is starting" variants={fade} initial="shown" animate="shown" exit="gone">
      <span className="splash-glow" aria-hidden="true" />
      <motion.img className="splash-logo" src={logo} alt="" initial={{ opacity: 0, scale: 0.3 }} animate={{ opacity: 1, scale: 1 }} transition={bouncy} />
      <motion.span className="splash-name serif" initial={{ opacity: 0, y: 8 }} animate={{ opacity: 1, y: 0 }} transition={{ ...bouncy, delay: 0.2 }}>
        Shodh
      </motion.span>
      <span className="splash-line">Connecting to your computer</span>
    </motion.div>
  );
}
