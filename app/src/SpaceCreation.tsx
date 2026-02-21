import React, { useState, useRef, useEffect } from 'react';
import './SpaceCreation.css';

interface SpaceCreationProps {
  onComplete: (space: NewSpace) => void;
  onCancel: () => void;
  existingSpaces: string[];
}

interface NewSpace {
  name: string;
  emoji: string;
  description?: string;
  color?: string;
}

const SpaceCreation: React.FC<SpaceCreationProps> = ({
  onComplete,
  onCancel,
  existingSpaces
}) => {
  const [spaceName, setSpaceName] = useState('');
  const [selectedEmoji, setSelectedEmoji] = useState('📁');
  const [selectedColor, setSelectedColor] = useState('#7FD4BC');
  const [description, setDescription] = useState('');
  const [nameError, setNameError] = useState('');
  const [isAnimating, setIsAnimating] = useState(false);
  
  const nameInputRef = useRef<HTMLInputElement>(null);
  const modalRef = useRef<HTMLDivElement>(null);

  const popularEmojis = [
    { emoji: '📁', label: 'Folder', category: 'general' },
    { emoji: '📚', label: 'Books', category: 'education' },
    { emoji: '🎓', label: 'Education', category: 'education' },
    { emoji: '💼', label: 'Work', category: 'professional' },
    { emoji: '🚀', label: 'Project', category: 'professional' },
    { emoji: '💡', label: 'Ideas', category: 'creative' },
    { emoji: '🔬', label: 'Research', category: 'science' },
    { emoji: '🎨', label: 'Creative', category: 'creative' },
    { emoji: '📝', label: 'Notes', category: 'general' },
    { emoji: '💻', label: 'Code', category: 'tech' },
    { emoji: '🏢', label: 'Business', category: 'professional' },
    { emoji: '🌟', label: 'Favorites', category: 'general' },
    { emoji: '🔥', label: 'Hot', category: 'general' },
    { emoji: '⚡', label: 'Quick', category: 'general' },
    { emoji: '🎯', label: 'Goals', category: 'professional' },
    { emoji: '🛠️', label: 'Tools', category: 'tech' },
    { emoji: '📊', label: 'Data', category: 'professional' },
    { emoji: '🌍', label: 'Global', category: 'general' },
    { emoji: '🔒', label: 'Private', category: 'general' },
    { emoji: '👥', label: 'Team', category: 'professional' },
    { emoji: '📸', label: 'Photos', category: 'creative' },
    { emoji: '🎵', label: 'Music', category: 'creative' },
    { emoji: '🎮', label: 'Gaming', category: 'entertainment' },
    { emoji: '🍳', label: 'Recipes', category: 'personal' }
  ];

  const colorPalette = [
    '#7FD4BC', // Mint green (complementary)
    '#FFB3D9', // Rose pink
    '#A8C9FF', // Sky blue  
    '#D4B5F0', // Lavender purple
    '#FFCC99', // Peach orange
    '#FFE680', // Soft yellow
  ];

  const templates = [
    { name: 'Personal Knowledge', emoji: '🧠', color: '#D4B5F0', description: 'Personal notes and learning' },
    { name: 'Work Projects', emoji: '💼', color: '#A8C9FF', description: 'Professional documents and tasks' },
    { name: 'Research Papers', emoji: '🔬', color: '#7FD4BC', description: 'Academic papers and citations' },
    { name: 'Creative Ideas', emoji: '💡', color: '#FFE680', description: 'Brainstorming and inspiration' },
    { name: 'Code Repository', emoji: '💻', color: '#FFCC99', description: 'Code snippets and documentation' },
  ];

  useEffect(() => {
    // Focus on name input when component mounts
    setTimeout(() => {
      nameInputRef.current?.focus();
    }, 100);

    // Handle click outside to close
    const handleClickOutside = (event: MouseEvent) => {
      if (modalRef.current && !modalRef.current.contains(event.target as Node)) {
        onCancel();
      }
    };

    // Handle escape key
    const handleEscape = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        onCancel();
      }
    };

    document.addEventListener('mousedown', handleClickOutside);
    document.addEventListener('keydown', handleEscape);

    return () => {
      document.removeEventListener('mousedown', handleClickOutside);
      document.removeEventListener('keydown', handleEscape);
    };
  }, [onCancel]);

  const handleNameChange = (e: React.ChangeEvent<HTMLInputElement>) => {
    const name = e.target.value;
    setSpaceName(name);
    
    // Validate name
    if (name.trim().length === 0) {
      setNameError('');
    } else if (name.trim().length < 2) {
      setNameError('Name must be at least 2 characters');
    } else if (existingSpaces.includes(name.trim())) {
      setNameError('A space with this name already exists');
    } else {
      setNameError('');
    }
  };

  const handleTemplateSelect = (template: typeof templates[0]) => {
    setSpaceName(template.name);
    setSelectedEmoji(template.emoji);
    setSelectedColor(template.color);
    setDescription(template.description);
  };

  const handleCreate = () => {
    if (spaceName.trim().length >= 2 && !nameError) {
      onComplete({
        name: spaceName.trim(),
        emoji: selectedEmoji,
        color: selectedColor,
        description: description.trim()
      });
    }
  };

  const isValid = spaceName.trim().length >= 2 && !nameError;

  return (
    <div className="space-creation-overlay">
      <div className="space-creation-modal" ref={modalRef}>
        
        {/* Header */}
        <div className="creation-header">
          <h2>Create New Space</h2>
          <p className="creation-subtitle">Organize your documents in a dedicated workspace</p>
          <button className="close-btn" onClick={onCancel} aria-label="Close">
            <svg width="20" height="20" viewBox="0 0 20 20" fill="currentColor">
              <path d="M14.95 5.05a.75.75 0 0 0-1.06 0L10 8.94 6.11 5.05a.75.75 0 0 0-1.06 1.06L8.94 10l-3.89 3.89a.75.75 0 1 0 1.06 1.06L10 11.06l3.89 3.89a.75.75 0 0 0 1.06-1.06L11.06 10l3.89-3.89a.75.75 0 0 0 0-1.06z"/>
            </svg>
          </button>
        </div>

        {/* Preview */}
        <div className="space-preview" style={{ background: `linear-gradient(135deg, ${selectedColor}20 0%, ${selectedColor}10 100%)` }}>
          <div className="preview-icon" style={{ background: selectedColor }}>
            <span className="preview-emoji">{selectedEmoji}</span>
          </div>
          <div className="preview-details">
            <div className="preview-name">{spaceName || 'Untitled Space'}</div>
            {description && <div className="preview-description">{description}</div>}
          </div>
        </div>

        {/* Templates */}
        <div className="templates-section">
          <h3>Quick Start Templates</h3>
          <div className="templates-grid">
            {templates.map((template, idx) => (
              <button
                key={idx}
                className={`template-card ${spaceName === template.name ? 'selected' : ''}`}
                onClick={() => handleTemplateSelect(template)}
              >
                <span className="template-emoji">{template.emoji}</span>
                <span className="template-name">{template.name}</span>
              </button>
            ))}
          </div>
        </div>

        {/* Form */}
        <div className="creation-form">
          {/* Name Input */}
          <div className="form-group">
            <label htmlFor="space-name">Space Name</label>
            <input
              ref={nameInputRef}
              id="space-name"
              type="text"
              value={spaceName}
              onChange={handleNameChange}
              placeholder="Enter a unique name"
              className={nameError ? 'error' : ''}
              maxLength={50}
            />
            {nameError && <span className="error-message">{nameError}</span>}
            <span className="character-count">{spaceName.length}/50</span>
          </div>

          {/* Description */}
          <div className="form-group">
            <label htmlFor="space-description">Description (Optional)</label>
            <textarea
              id="space-description"
              value={description}
              onChange={(e) => setDescription(e.target.value)}
              placeholder="What will this space be used for?"
              rows={2}
              maxLength={200}
            />
            <span className="character-count">{description.length}/200</span>
          </div>

          {/* Emoji Selection */}
          <div className="form-group">
            <label>Choose an Icon</label>
            <div className="emoji-grid">
              {popularEmojis.map((item, idx) => (
                <button
                  key={idx}
                  className={`emoji-option ${selectedEmoji === item.emoji ? 'selected' : ''}`}
                  onClick={() => {
                    setSelectedEmoji(item.emoji);
                  }}
                  title={item.label}
                >
                  <span className="emoji-char">{item.emoji}</span>
                </button>
              ))}
            </div>
          </div>

          {/* Color Selection */}
          <div className="form-group">
            <label>Accent Color</label>
            <div className="color-grid">
              {colorPalette.map((color, idx) => (
                <button
                  key={idx}
                  className={`color-option ${selectedColor === color ? 'selected' : ''}`}
                  style={{ background: color }}
                  onClick={() => {
                    setSelectedColor(color);
                  }}
                >
                  {selectedColor === color && (
                    <svg className="color-check" viewBox="0 0 24 24">
                      <path
                        fill="currentColor"
                        d="M9 16.17L4.83 12l-1.42 1.41L9 19 21 7l-1.41-1.41L9 16.17z"
                      />
                    </svg>
                  )}
                </button>
              ))}
            </div>
          </div>
        </div>

        {/* Actions */}
        <div className="creation-actions">
          <button className="btn-cancel" onClick={onCancel}>
            Cancel
          </button>
          <button 
            className={`btn-create ${isValid ? 'ready' : ''}`}
            onClick={handleCreate}
            disabled={!isValid}
          >
            <span className="btn-text">Create Space</span>
            {isValid && <span className="btn-icon">→</span>}
          </button>
        </div>
      </div>
    </div>
  );
};

export default SpaceCreation;