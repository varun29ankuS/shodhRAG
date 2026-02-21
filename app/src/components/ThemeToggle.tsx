import { Moon, Sun } from 'lucide-react';
import { motion } from 'framer-motion';
import { useTheme } from '../contexts/ThemeContext';

export const ThemeToggle = () => {
  const { theme, toggleTheme } = useTheme();

  return (
    <motion.button
      whileHover={{ scale: 1.05 }}
      whileTap={{ scale: 0.95 }}
      onClick={toggleTheme}
      className="relative w-14 h-7 rounded-full transition-colors duration-300 flex items-center px-1"
      style={{
        backgroundColor: theme === 'light' ? '#e5e7eb' : '#374151',
      }}
      aria-label="Toggle theme"
    >
      <motion.div
        className="w-5 h-5 rounded-full flex items-center justify-center"
        style={{
          backgroundColor: theme === 'light' ? '#ff6b35' : '#ff6b35',
        }}
        animate={{
          x: theme === 'light' ? 0 : 28,
        }}
        transition={{
          type: 'spring',
          stiffness: 500,
          damping: 30,
        }}
      >
        {theme === 'light' ? (
          <Sun className="w-3 h-3 text-white" />
        ) : (
          <Moon className="w-3 h-3 text-white" />
        )}
      </motion.div>

      {/* Background icons */}
      <div className="absolute inset-0 flex items-center justify-between px-2 pointer-events-none">
        <Sun className="w-3 h-3 text-gray-400" style={{ opacity: theme === 'light' ? 0 : 0.5 }} />
        <Moon className="w-3 h-3 text-gray-400" style={{ opacity: theme === 'dark' ? 0 : 0.5 }} />
      </div>
    </motion.button>
  );
};
