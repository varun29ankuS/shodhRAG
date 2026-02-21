import { useState, useEffect } from 'react';
import './ThemeSettings.css';

interface ThemeSettingsProps {
  isOpen: boolean;
  onClose: () => void;
}

const ThemeSettings: React.FC<ThemeSettingsProps> = ({ isOpen, onClose }) => {
  const [theme, setTheme] = useState<'dark' | 'light'>('dark');
  const [accentColor, setAccentColor] = useState<'green' | 'blood-red' | 'maroon' | 'yellow'>('green');

  useEffect(() => {
    // Load saved preferences
    const savedTheme = localStorage.getItem('theme') as 'dark' | 'light' || 'dark';
    const savedAccent = localStorage.getItem('accentColor') as 'green' | 'blood-red' | 'maroon' | 'yellow' || 'green';
    
    setTheme(savedTheme);
    setAccentColor(savedAccent);
    
    // Apply theme to document
    document.documentElement.setAttribute('data-theme', savedTheme);
    document.documentElement.setAttribute('data-accent', savedAccent);
  }, []);

  const handleThemeChange = (newTheme: 'dark' | 'light') => {
    setTheme(newTheme);
    localStorage.setItem('theme', newTheme);
    document.documentElement.setAttribute('data-theme', newTheme);
  };

  const handleAccentChange = (newAccent: 'green' | 'blood-red' | 'maroon' | 'yellow') => {
    setAccentColor(newAccent);
    localStorage.setItem('accentColor', newAccent);
    document.documentElement.setAttribute('data-accent', newAccent);
  };

  if (!isOpen) return null;

  return (
    <div className="theme-settings-overlay" onClick={onClose}>
      <div className="theme-settings-modal" onClick={(e) => e.stopPropagation()}>
        <div className="theme-settings-header">
          <h2>🎨 Appearance</h2>
          <button className="close-btn" onClick={onClose}>✕</button>
        </div>

        <div className="theme-settings-content">
          {/* Theme Toggle */}
          <div className="setting-group">
            <h3>Theme</h3>
            <div className="theme-options">
              <button
                className={`theme-option ${theme === 'dark' ? 'active' : ''}`}
                onClick={() => handleThemeChange('dark')}
              >
                <span className="theme-icon">🌙</span>
                <span>Dark</span>
              </button>
              <button
                className={`theme-option ${theme === 'light' ? 'active' : ''}`}
                onClick={() => handleThemeChange('light')}
              >
                <span className="theme-icon">☀️</span>
                <span>Light</span>
              </button>
            </div>
          </div>

          {/* Accent Color */}
          <div className="setting-group">
            <h3>Accent Color</h3>
            <div className="accent-options">
              <button
                className={`accent-option ${accentColor === 'green' ? 'active' : ''}`}
                onClick={() => handleAccentChange('green')}
                style={{ '--preview-color': '#00D4AA' } as React.CSSProperties}
              >
                <span className="color-preview green"></span>
                <span>Green</span>
              </button>
              <button
                className={`accent-option ${accentColor === 'blood-red' ? 'active' : ''}`}
                onClick={() => handleAccentChange('blood-red')}
                style={{ '--preview-color': '#DC143C' } as React.CSSProperties}
              >
                <span className="color-preview blood-red"></span>
                <span>Blood Red</span>
              </button>
              <button
                className={`accent-option ${accentColor === 'maroon' ? 'active' : ''}`}
                onClick={() => handleAccentChange('maroon')}
                style={{ '--preview-color': '#800020' } as React.CSSProperties}
              >
                <span className="color-preview maroon"></span>
                <span>Maroon</span>
              </button>
              <button
                className={`accent-option ${accentColor === 'yellow' ? 'active' : ''}`}
                onClick={() => handleAccentChange('yellow')}
                style={{ '--preview-color': '#FFD700' } as React.CSSProperties}
              >
                <span className="color-preview yellow"></span>
                <span>Yellow</span>
              </button>
            </div>
          </div>

          {/* Preview */}
          <div className="setting-group">
            <h3>Preview</h3>
            <div className="preview-box">
              <div className="preview-text">The quick brown fox jumps over the lazy dog</div>
              <button className="preview-button">Sample Button</button>
            </div>
          </div>
        </div>
      </div>
    </div>
  );
};

export default ThemeSettings;