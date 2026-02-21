import React, { useState, useEffect, useRef } from 'react';
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { listen } from "@tauri-apps/api/event";
import './FolderLinking.css';

interface FolderLinkingProps {
  spaceName: string;
  spaceId: string;
  onComplete: (result: IndexingResult) => void;
  onCancel: () => void;
}

interface FileInfo {
  path: string;
  name: string;
  type: string;
  size: number;
  selected: boolean;
}

interface FolderPreview {
  path: string;
  total_files?: number;  // Rust sends snake_case
  totalFiles?: number;    // TypeScript expects camelCase
  files_by_type?: Record<string, number>;
  filesByType?: Record<string, number>;
  estimated_time?: number;
  estimatedTime?: number;
  files: FileInfo[];
}

interface IndexingProgress {
  current_file?: string;
  currentFile?: string;
  processed_files?: number;
  processedFiles?: number;
  total_files?: number;
  totalFiles?: number;
  percentage: number;
  current_action?: string;
  currentAction?: string;
  eta_seconds?: number;
  etaSeconds?: number;
  speed: number;
}

interface IndexingResult {
  files_processed?: number;
  filesProcessed?: number;
  total_chunks?: number;
  totalChunks?: number;
  failed_files?: string[];
  failedFiles?: string[];
  duration: number;
}

type Step = 'select' | 'preview' | 'options' | 'processing' | 'complete';

const FolderLinking: React.FC<FolderLinkingProps> = ({
  spaceName,
  spaceId,
  onComplete,
  onCancel
}) => {
  const [currentStep, setCurrentStep] = useState<Step>('select');
  const [selectedFolder, setSelectedFolder] = useState<string>('');
  const [folderPreview, setFolderPreview] = useState<FolderPreview | null>(null);
  const [progress, setProgress] = useState<IndexingProgress>({
    currentFile: '',
    processedFiles: 0,
    totalFiles: 0,
    percentage: 0,
    currentAction: 'Initializing...',
    etaSeconds: 0,
    speed: 0
  });
  const [result, setResult] = useState<IndexingResult | null>(null);
  const [isPaused, setIsPaused] = useState(false);
  const [selectedTypes, setSelectedTypes] = useState<Set<string>>(new Set(['pdf', 'txt', 'md', 'docx']));
  const [options, setOptions] = useState({
    skipIndexed: true,
    watchChanges: true,
    processSubdirs: true,
    priority: 'normal' as 'high' | 'normal' | 'low'
  });
  const [error, setError] = useState<string | null>(null);
  
  const modalRef = useRef<HTMLDivElement>(null);
  const progressBarRef = useRef<HTMLDivElement>(null);
  const animationFrameRef = useRef<number>(0);
  const unlistenRef = useRef<any>(null);

  useEffect(() => {
    // Listen for progress updates from backend
    const setupListener = async () => {
      console.log('Setting up indexing-progress listener');
      unlistenRef.current = await listen('indexing-progress', (event: any) => {
        console.log('Received progress event:', event.payload);
        const progressData = event.payload as IndexingProgress;
        
        // Map snake_case from Rust to camelCase for TypeScript
        setProgress({
          currentFile: progressData.currentFile || progressData.current_file || '',
          processedFiles: progressData.processedFiles || progressData.processed_files || 0,
          totalFiles: progressData.totalFiles || progressData.total_files || 0,
          percentage: progressData.percentage || 0,
          currentAction: progressData.currentAction || progressData.current_action || 'Processing...',
          etaSeconds: progressData.etaSeconds || progressData.eta_seconds || 0,
          speed: progressData.speed || 0
        });
      });
      console.log('Listener setup complete');
    };
    
    setupListener();

    // Handle click outside to close
    const handleClickOutside = (event: MouseEvent) => {
      if (modalRef.current && !modalRef.current.contains(event.target as Node)) {
        // Only allow closing on select step or complete
        if (currentStep === 'select' || currentStep === 'complete') {
          onCancel();
        }
      }
    };

    // Handle escape key
    const handleEscape = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        // Only allow closing on select step or complete
        if (currentStep === 'select' || currentStep === 'complete') {
          onCancel();
        }
      }
    };

    document.addEventListener('mousedown', handleClickOutside);
    document.addEventListener('keydown', handleEscape);
    
    return () => {
      if (unlistenRef.current) {
        unlistenRef.current();
      }
      if (animationFrameRef.current) {
        cancelAnimationFrame(animationFrameRef.current);
      }
      document.removeEventListener('mousedown', handleClickOutside);
      document.removeEventListener('keydown', handleEscape);
    };
  }, [currentStep, onCancel]);

  const handleSelectFolder = async () => {
    try {
      const selected = await open({
        directory: true,
        multiple: false,
        title: `Select folder for ${spaceName}`
      });
      
      if (selected) {
        setSelectedFolder(selected as string);
        // Get folder preview
        const preview = await invoke<FolderPreview>('preview_folder', {
          folderPath: selected
        });
        
        // Map snake_case to camelCase
        setFolderPreview({
          path: preview.path,
          totalFiles: preview.totalFiles || preview.total_files || 0,
          filesByType: preview.filesByType || preview.files_by_type || {},
          estimatedTime: preview.estimatedTime || preview.estimated_time || 0,
          files: preview.files || []
        });
        setCurrentStep('preview');
      }
    } catch (error) {
      console.error('Error selecting folder:', error);
    }
  };

  const handleStartIndexing = async () => {
    setCurrentStep('processing');
    setError(null);
    
    try {
      console.log('Starting indexing with params:', {
        folderPath: selectedFolder,
        spaceId,
        options: {
          skip_indexed: options.skipIndexed,
          watch_changes: options.watchChanges,
          process_subdirs: options.processSubdirs,
          priority: options.priority,
          file_types: Array.from(selectedTypes)
        }
      });

      const result = await invoke<IndexingResult>('link_folder_enhanced', {
        folderPath: selectedFolder,
        spaceId,
        options: {
          skip_indexed: options.skipIndexed,
          watch_changes: options.watchChanges,
          process_subdirs: options.processSubdirs,
          priority: options.priority,
          file_types: Array.from(selectedTypes)
        }
      });
      
      console.log('Indexing completed:', result);
      
      // Map snake_case to camelCase
      setResult({
        filesProcessed: result.filesProcessed || result.files_processed || 0,
        totalChunks: result.totalChunks || result.total_chunks || 0,
        failedFiles: result.failedFiles || result.failed_files || [],
        duration: result.duration || 0
      });
      setCurrentStep('complete');
    } catch (error) {
      console.error('Error indexing folder:', error);
      setError(error?.toString() || 'Failed to index folder');
      // Don't leave user on blank screen - go back to options
      setCurrentStep('options');
    }
  };

  const handlePauseResume = async () => {
    if (isPaused) {
      await invoke('resume_indexing');
    } else {
      await invoke('pause_indexing');
    }
    setIsPaused(!isPaused);
  };

  const handleCancel = async () => {
    await invoke('cancel_indexing');
    onCancel();
  };

  const formatTime = (seconds: number): string => {
    if (seconds < 60) return `${Math.round(seconds)}s`;
    const minutes = Math.floor(seconds / 60);
    const secs = Math.round(seconds % 60);
    return `${minutes}m ${secs}s`;
  };

  const formatFileSize = (bytes: number): string => {
    if (bytes < 1024) return `${bytes} B`;
    if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
    if (bytes < 1024 * 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
    return `${(bytes / (1024 * 1024 * 1024)).toFixed(1)} GB`;
  };

  const getFileIcon = (type: string): string => {
    const icons: Record<string, string> = {
      pdf: '📄',
      txt: '📝',
      md: '📑',
      docx: '📋',
      xlsx: '📊',
      code: '💻',
      image: '🖼️',
      default: '📎'
    };
    return icons[type] || icons.default;
  };

  return (
    <div className="folder-linking-overlay">
      <div className={`folder-linking-modal step-${currentStep}`} ref={modalRef}>
        {/* Close button */}
        {(currentStep === 'select' || currentStep === 'complete') && (
          <button className="close-btn" onClick={onCancel} aria-label="Close">
            <svg width="20" height="20" viewBox="0 0 20 20" fill="currentColor">
              <path d="M14.95 5.05a.75.75 0 0 0-1.06 0L10 8.94 6.11 5.05a.75.75 0 0 0-1.06 1.06L8.94 10l-3.89 3.89a.75.75 0 1 0 1.06 1.06L10 11.06l3.89 3.89a.75.75 0 0 0 1.06-1.06L11.06 10l3.89-3.89a.75.75 0 0 0 0-1.06z"/>
            </svg>
          </button>
        )}
        
        {/* Step Indicator */}
        <div className="step-indicator">
          <div className={`step ${currentStep === 'select' ? 'active' : ''} ${['preview', 'options', 'processing', 'complete'].includes(currentStep) ? 'completed' : ''}`}>
            <div className="step-circle">1</div>
            <span>Select</span>
          </div>
          <div className="step-line"></div>
          <div className={`step ${currentStep === 'preview' ? 'active' : ''} ${['options', 'processing', 'complete'].includes(currentStep) ? 'completed' : ''}`}>
            <div className="step-circle">2</div>
            <span>Preview</span>
          </div>
          <div className="step-line"></div>
          <div className={`step ${currentStep === 'options' ? 'active' : ''} ${['processing', 'complete'].includes(currentStep) ? 'completed' : ''}`}>
            <div className="step-circle">3</div>
            <span>Options</span>
          </div>
          <div className="step-line"></div>
          <div className={`step ${currentStep === 'processing' ? 'active' : ''} ${currentStep === 'complete' ? 'completed' : ''}`}>
            <div className="step-circle">4</div>
            <span>Process</span>
          </div>
        </div>

        {/* Step 1: Select Folder */}
        {currentStep === 'select' && (
          <div className="step-content fade-in">
            <div className="folder-select-icon">
              <div className="folder-icon-animated">
                <svg viewBox="0 0 100 100" className="folder-svg">
                  <path d="M10 30 Q10 25 15 25 L35 25 L40 20 Q42 15 47 15 L85 15 Q90 15 90 20 L90 80 Q90 85 85 85 L15 85 Q10 85 10 80 Z" 
                        fill="url(#folderGradient)" 
                        stroke="rgba(59, 130, 246, 0.5)" 
                        strokeWidth="2"/>
                  <defs>
                    <linearGradient id="folderGradient" x1="0%" y1="0%" x2="100%" y2="100%">
                      <stop offset="0%" stopColor="#60A5FA" />
                      <stop offset="100%" stopColor="#3B82F6" />
                    </linearGradient>
                  </defs>
                </svg>
                <div className="plus-icon">+</div>
              </div>
            </div>
            <h2>Link a Folder to {spaceName}</h2>
            <p className="subtitle">Select a folder to automatically sync and index its contents</p>
            <button className="btn-primary large" onClick={handleSelectFolder}>
              Choose Folder
            </button>
            <button className="btn-text" onClick={onCancel}>Cancel</button>
          </div>
        )}

        {/* Step 2: Preview */}
        {currentStep === 'preview' && folderPreview && (
          <div className="step-content fade-in">
            <h2>Folder Contents</h2>
            <div className="folder-path">{selectedFolder}</div>
            
            <div className="preview-stats">
              <div className="stat-card">
                <div className="stat-value">{folderPreview.totalFiles}</div>
                <div className="stat-label">Total Files</div>
              </div>
              <div className="stat-card">
                <div className="stat-value">{formatTime(folderPreview.estimatedTime || 0)}</div>
                <div className="stat-label">Estimated Time</div>
              </div>
            </div>

            <div className="file-types">
              <h3>File Types</h3>
              <div className="type-grid">
                {Object.entries(folderPreview.filesByType || {}).map(([type, count]) => (
                  <label key={type} className="type-card">
                    <input
                      type="checkbox"
                      checked={selectedTypes.has(type)}
                      onChange={(e) => {
                        const newTypes = new Set(selectedTypes);
                        if (e.target.checked) {
                          newTypes.add(type);
                        } else {
                          newTypes.delete(type);
                        }
                        setSelectedTypes(newTypes);
                      }}
                    />
                    <div className="type-content">
                      <span className="type-icon">{getFileIcon(type)}</span>
                      <span className="type-name">{type.toUpperCase()}</span>
                      <span className="type-count">{count} files</span>
                    </div>
                  </label>
                ))}
              </div>
            </div>

            <div className="button-group">
              <button className="btn-secondary" onClick={() => setCurrentStep('select')}>
                Back
              </button>
              <button 
                className="btn-primary" 
                onClick={() => setCurrentStep('options')}
                disabled={selectedTypes.size === 0}
              >
                Continue
              </button>
            </div>
          </div>
        )}

        {/* Step 3: Options */}
        {currentStep === 'options' && (
          <div className="step-content fade-in">
            <h2>Processing Options</h2>
            
            {error && (
              <div style={{
                padding: '12px',
                background: 'rgba(255, 94, 91, 0.1)',
                border: '1px solid rgba(255, 94, 91, 0.3)',
                borderRadius: '8px',
                color: 'var(--error)',
                marginBottom: '16px',
                fontSize: '14px'
              }}>
                ⚠️ {error}
              </div>
            )}
            
            <div className="options-list">
              <label className="option-item">
                <input
                  type="checkbox"
                  checked={options.skipIndexed}
                  onChange={(e) => setOptions({...options, skipIndexed: e.target.checked})}
                />
                <div className="option-content">
                  <div className="option-title">Skip Already Indexed Files</div>
                  <div className="option-description">Only process new or modified files</div>
                </div>
              </label>

              <label className="option-item">
                <input
                  type="checkbox"
                  checked={options.watchChanges}
                  onChange={(e) => setOptions({...options, watchChanges: e.target.checked})}
                />
                <div className="option-content">
                  <div className="option-title">Watch for Changes</div>
                  <div className="option-description">Automatically index new files added to this folder</div>
                </div>
              </label>

              <label className="option-item">
                <input
                  type="checkbox"
                  checked={options.processSubdirs}
                  onChange={(e) => setOptions({...options, processSubdirs: e.target.checked})}
                />
                <div className="option-content">
                  <div className="option-title">Include Subdirectories</div>
                  <div className="option-description">Process files in all subdirectories</div>
                </div>
              </label>
            </div>

            <div className="priority-section">
              <h3>Processing Priority</h3>
              <div className="priority-options">
                {(['high', 'normal', 'low'] as const).map((priority) => (
                  <label key={priority} className={`priority-option ${options.priority === priority ? 'selected' : ''}`}>
                    <input
                      type="radio"
                      name="priority"
                      value={priority}
                      checked={options.priority === priority}
                      onChange={() => setOptions({...options, priority})}
                    />
                    <span className="priority-label">
                      {priority === 'high' && '⚡'}
                      {priority === 'normal' && '⚙️'}
                      {priority === 'low' && '🐌'}
                      {priority.charAt(0).toUpperCase() + priority.slice(1)}
                    </span>
                  </label>
                ))}
              </div>
            </div>

            <div className="button-group">
              <button className="btn-secondary" onClick={() => setCurrentStep('preview')}>
                Back
              </button>
              <button className="btn-primary" onClick={handleStartIndexing}>
                Start Indexing
              </button>
            </div>
          </div>
        )}

        {/* Step 4: Processing */}
        {currentStep === 'processing' && (
          <div className="step-content fade-in">
            <h2>Indexing in Progress</h2>
            
            {progress.totalFiles === 0 ? (
              <div style={{ textAlign: 'center', padding: '40px 0' }}>
                <div className="spinner" style={{
                  width: '48px',
                  height: '48px',
                  border: '3px solid var(--border)',
                  borderTopColor: 'var(--accent)',
                  borderRadius: '50%',
                  animation: 'spin 1s linear infinite',
                  margin: '0 auto 20px'
                }}></div>
                <p style={{ color: 'var(--text-dim)', fontSize: '14px' }}>
                  Initializing indexing process...
                </p>
                <p style={{ color: 'var(--text-dimmer)', fontSize: '12px', marginTop: '8px' }}>
                  This may take a few moments to start
                </p>
              </div>
            ) : (
              <>
                <p className="current-file">{progress.currentFile || 'Processing...'}</p>
                
                <div className="progress-container">
                  <div className="progress-stats">
                    <span>{progress.processedFiles} of {progress.totalFiles} files</span>
                    <span>{Math.round(progress.percentage)}%</span>
                  </div>
                  <div className="progress-bar">
                    <div 
                      className="progress-fill" 
                      style={{ width: `${progress.percentage}%` }}
                      ref={progressBarRef}
                    >
                      <div className="progress-glow"></div>
                    </div>
                  </div>
                  <div className="progress-info">
                    <span className="progress-action">{progress.currentAction}</span>
                    <span className="progress-eta">
                      {progress.etaSeconds && progress.etaSeconds > 0 && `${formatTime(progress.etaSeconds)} remaining`}
                    </span>
                  </div>
                </div>
              </>
            )}

            {progress.speed > 0 && (
              <div className="speed-indicator">
                Processing at {progress.speed.toFixed(1)} files/sec
              </div>
            )}

            <div className="button-group">
              <button 
                className="btn-secondary" 
                onClick={handlePauseResume}
              >
                {isPaused ? '▶️ Resume' : '⏸️ Pause'}
              </button>
              <button className="btn-text" onClick={handleCancel}>
                Cancel
              </button>
            </div>
          </div>
        )}

        {/* Step 5: Complete */}
        {currentStep === 'complete' && result && (
          <div className="step-content fade-in">
            <div className="success-animation">
              <svg className="checkmark" viewBox="0 0 52 52">
                <circle className="checkmark-circle" cx="26" cy="26" r="25" fill="none"/>
                <path className="checkmark-check" fill="none" d="M14.1 27.2l7.1 7.2 16.7-16.8"/>
              </svg>
            </div>

            <h2>Indexing Complete!</h2>
            <p className="subtitle">Your folder has been successfully linked and indexed</p>

            <div className="result-stats">
              <div className="stat-card success">
                <div className="stat-value">{result.filesProcessed}</div>
                <div className="stat-label">Files Processed</div>
              </div>
              <div className="stat-card">
                <div className="stat-value">{result.totalChunks}</div>
                <div className="stat-label">Chunks Created</div>
              </div>
              <div className="stat-card">
                <div className="stat-value">{formatTime(result.duration / 1000)}</div>
                <div className="stat-label">Time Taken</div>
              </div>
            </div>

            {result.failedFiles && result.failedFiles.length > 0 && (
              <div className="failed-files">
                <h3>⚠️ Failed Files ({result.failedFiles.length})</h3>
                <div className="failed-list">
                  {result.failedFiles.slice(0, 5).map((file, idx) => (
                    <div key={idx} className="failed-file">{file}</div>
                  ))}
                  {result.failedFiles.length > 5 && (
                    <div className="failed-more">
                      and {result.failedFiles.length - 5} more...
                    </div>
                  )}
                </div>
              </div>
            )}

            <button 
              className="btn-primary large" 
              onClick={() => onComplete(result)}
            >
              Done
            </button>
          </div>
        )}
      </div>
    </div>
  );
};

export default FolderLinking;