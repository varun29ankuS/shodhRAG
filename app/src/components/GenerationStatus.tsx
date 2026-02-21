import React, { useState, useEffect } from 'react';
import './GenerationStatus.css';

export interface GenerationStep {
  id: string;
  name: string;
  description: string;
  status: 'pending' | 'running' | 'complete' | 'error';
  progress?: number;
  result?: any;
  duration?: number;
}

interface GenerationStatusProps {
  steps: GenerationStep[];
  onComplete?: (document: any) => void;
  onError?: (error: string) => void;
  compact?: boolean;
}

export const GenerationStatus: React.FC<GenerationStatusProps> = ({
  steps,
  onComplete,
  onError,
  compact = false
}) => {
  const [expandedSteps, setExpandedSteps] = useState<Set<string>>(new Set());
  
  const toggleStep = (stepId: string) => {
    setExpandedSteps(prev => {
      const next = new Set(prev);
      if (next.has(stepId)) {
        next.delete(stepId);
      } else {
        next.add(stepId);
      }
      return next;
    });
  };

  const getStepIcon = (status: string) => {
    switch (status) {
      case 'complete': return '✅';
      case 'running': return '⚡';
      case 'pending': return '⏳';
      case 'error': return '❌';
      default: return '○';
    }
  };

  const totalSteps = steps.length;
  const completedSteps = steps.filter(s => s.status === 'complete').length;
  const overallProgress = totalSteps > 0 ? (completedSteps / totalSteps) * 100 : 0;

  if (compact) {
    // Compact inline view for chat
    return (
      <div className="generation-status-compact">
        {steps.map(step => (
          <div key={step.id} className={`status-line ${step.status}`}>
            <span className="status-icon">{getStepIcon(step.status)}</span>
            <span className="status-name">{step.name}</span>
            {step.status === 'running' && step.progress && (
              <span className="status-progress">{step.progress}%</span>
            )}
          </div>
        ))}
      </div>
    );
  }

  // Full detailed view
  return (
    <div className="generation-status">
      <div className="generation-header">
        <h3>Generating Document</h3>
        <div className="overall-progress">
          <div className="progress-bar">
            <div 
              className="progress-fill"
              style={{ width: `${overallProgress}%` }}
            />
          </div>
          <span className="progress-text">
            {completedSteps} of {totalSteps} steps
          </span>
        </div>
      </div>
      
      <div className="generation-steps">
        {steps.map(step => (
          <div 
            key={step.id} 
            className={`generation-step ${step.status}`}
            onClick={() => toggleStep(step.id)}
          >
            <div className="step-header">
              <span className="step-icon">{getStepIcon(step.status)}</span>
              <div className="step-info">
                <div className="step-name">{step.name}</div>
                <div className="step-description">{step.description}</div>
              </div>
              {step.duration && (
                <span className="step-duration">{step.duration}ms</span>
              )}
            </div>
            
            {step.status === 'running' && step.progress !== undefined && (
              <div className="step-progress">
                <div className="step-progress-bar">
                  <div 
                    className="step-progress-fill"
                    style={{ width: `${step.progress}%` }}
                  />
                </div>
              </div>
            )}
            
            {expandedSteps.has(step.id) && step.result && (
              <div className="step-result">
                <pre>{JSON.stringify(step.result, null, 2)}</pre>
              </div>
            )}
          </div>
        ))}
      </div>
    </div>
  );
};

// Document card component for showing generated document in chat
export interface GeneratedDocumentCard {
  id: string;
  title: string;
  format: string;
  size: number;
  pages?: number;
  preview?: string;
}

interface DocumentCardProps {
  document: GeneratedDocumentCard;
  onPreview: () => void;
  onDownload: () => void;
  onRegenerate?: () => void;
}

export const DocumentCard: React.FC<DocumentCardProps> = ({
  document,
  onPreview,
  onDownload,
  onRegenerate
}) => {
  const formatFileSize = (bytes: number): string => {
    if (bytes === 0) return '0 B';
    const k = 1024;
    const sizes = ['B', 'KB', 'MB', 'GB'];
    const i = Math.floor(Math.log(bytes) / Math.log(k));
    return `${parseFloat((bytes / Math.pow(k, i)).toFixed(2))} ${sizes[i]}`;
  };

  const getFormatIcon = (format: string): string => {
    switch (format.toLowerCase()) {
      case 'pdf': return '📄';
      case 'docx':
      case 'doc': return '📝';
      case 'xlsx':
      case 'xls': return '📊';
      case 'pptx':
      case 'ppt': return '📽️';
      case 'csv': return '📈';
      case 'json': return '{ }';
      case 'md':
      case 'markdown': return '📑';
      case 'html': return '🌐';
      default: return '📄';
    }
  };

  return (
    <div className="generated-document-card">
      <div className="doc-card-icon">
        {getFormatIcon(document.format)}
      </div>
      
      <div className="doc-card-info">
        <div className="doc-card-title">{document.title}</div>
        <div className="doc-card-meta">
          <span>{formatFileSize(document.size)}</span>
          {document.pages && (
            <>
              <span className="meta-separator">•</span>
              <span>{document.pages} pages</span>
            </>
          )}
          <span className="meta-separator">•</span>
          <span>{document.format.toUpperCase()}</span>
        </div>
      </div>
      
      <div className="doc-card-actions">
        <button 
          className="doc-action-btn preview"
          onClick={onPreview}
          title="Preview"
        >
          👁️
        </button>
        <button 
          className="doc-action-btn download"
          onClick={onDownload}
          title="Download"
        >
          💾
        </button>
        {onRegenerate && (
          <button 
            className="doc-action-btn regenerate"
            onClick={onRegenerate}
            title="Regenerate"
          >
            🔄
          </button>
        )}
      </div>
    </div>
  );
};

export default GenerationStatus;