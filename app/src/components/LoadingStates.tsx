import React from 'react';
import './LoadingStates.css';

interface SkeletonProps {
  width?: string | number;
  height?: string | number;
  variant?: 'text' | 'circular' | 'rectangular' | 'rounded';
  animation?: 'pulse' | 'wave' | 'none';
  className?: string;
}

/**
 * Skeleton loader for content placeholders
 */
export const Skeleton: React.FC<SkeletonProps> = ({
  width = '100%',
  height = 20,
  variant = 'text',
  animation = 'pulse',
  className = '',
}) => {
  const classes = `skeleton skeleton-${variant} skeleton-${animation} ${className}`;
  
  const style: React.CSSProperties = {
    width: typeof width === 'number' ? `${width}px` : width,
    height: typeof height === 'number' ? `${height}px` : height,
  };

  return <div className={classes} style={style} />;
};

/**
 * Document skeleton for loading states
 */
export const DocumentSkeleton: React.FC = () => (
  <div className="document-skeleton">
    <div className="doc-skeleton-header">
      <Skeleton variant="circular" width={40} height={40} />
      <div className="doc-skeleton-info">
        <Skeleton width="60%" height={16} />
        <Skeleton width="40%" height={12} />
      </div>
    </div>
    <div className="doc-skeleton-content">
      <Skeleton width="100%" height={14} />
      <Skeleton width="90%" height={14} />
      <Skeleton width="75%" height={14} />
    </div>
  </div>
);

/**
 * Space skeleton for sidebar loading
 */
export const SpaceSkeleton: React.FC = () => (
  <div className="space-skeleton">
    <Skeleton variant="circular" width={32} height={32} />
    <div className="space-skeleton-info">
      <Skeleton width="70%" height={14} />
      <Skeleton width="50%" height={10} />
    </div>
  </div>
);

/**
 * Chat message skeleton
 */
export const ChatMessageSkeleton: React.FC = () => (
  <div className="chat-message-skeleton">
    <Skeleton variant="circular" width={36} height={36} />
    <div className="message-skeleton-content">
      <Skeleton width="80%" height={16} />
      <Skeleton width="60%" height={16} />
      <Skeleton width="40%" height={16} />
    </div>
  </div>
);

/**
 * Search result skeleton
 */
export const SearchResultSkeleton: React.FC = () => (
  <div className="search-result-skeleton">
    <div className="result-skeleton-header">
      <Skeleton width="50%" height={18} />
      <Skeleton width={60} height={20} variant="rounded" />
    </div>
    <Skeleton width="100%" height={14} />
    <Skeleton width="85%" height={14} />
    <Skeleton width="70%" height={14} />
  </div>
);

interface SpinnerProps {
  size?: 'small' | 'medium' | 'large';
  color?: string;
  className?: string;
}

/**
 * Loading spinner component
 */
export const Spinner: React.FC<SpinnerProps> = ({
  size = 'medium',
  color = 'var(--accent)',
  className = '',
}) => {
  const sizeMap = {
    small: 16,
    medium: 24,
    large: 32,
  };

  return (
    <div className={`spinner spinner-${size} ${className}`}>
      <svg
        width={sizeMap[size]}
        height={sizeMap[size]}
        viewBox="0 0 24 24"
        xmlns="http://www.w3.org/2000/svg"
      >
        <circle
          className="spinner-circle"
          cx="12"
          cy="12"
          r="10"
          fill="none"
          stroke={color}
          strokeWidth="2"
          strokeLinecap="round"
          strokeDasharray="60 200"
        />
      </svg>
    </div>
  );
};

interface LoadingOverlayProps {
  message?: string;
  progress?: number;
  transparent?: boolean;
}

/**
 * Full screen loading overlay
 */
export const LoadingOverlay: React.FC<LoadingOverlayProps> = ({
  message = 'Loading...',
  progress,
  transparent = false,
}) => (
  <div className={`loading-overlay ${transparent ? 'transparent' : ''}`}>
    <div className="loading-content">
      <Spinner size="large" />
      {message && <p className="loading-message">{message}</p>}
      {progress !== undefined && (
        <div className="loading-progress">
          <div className="progress-bar">
            <div 
              className="progress-fill" 
              style={{ width: `${Math.min(100, Math.max(0, progress))}%` }}
            />
          </div>
          <span className="progress-text">{Math.round(progress)}%</span>
        </div>
      )}
    </div>
  </div>
);

interface LoadingButtonProps {
  loading: boolean;
  children: React.ReactNode;
  onClick?: () => void;
  disabled?: boolean;
  className?: string;
  variant?: 'primary' | 'secondary' | 'danger';
}

/**
 * Button with loading state
 */
export const LoadingButton: React.FC<LoadingButtonProps> = ({
  loading,
  children,
  onClick,
  disabled,
  className = '',
  variant = 'primary',
}) => (
  <button
    className={`loading-button loading-button-${variant} ${className} ${loading ? 'loading' : ''}`}
    onClick={onClick}
    disabled={disabled || loading}
  >
    {loading && <Spinner size="small" color="currentColor" />}
    <span className={loading ? 'button-text-hidden' : ''}>{children}</span>
  </button>
);

interface LazyImageProps {
  src: string;
  alt: string;
  className?: string;
  onLoad?: () => void;
  onError?: () => void;
}

/**
 * Image with lazy loading and placeholders
 */
export const LazyImage: React.FC<LazyImageProps> = ({
  src,
  alt,
  className = '',
  onLoad,
  onError,
}) => {
  const [loaded, setLoaded] = React.useState(false);
  const [error, setError] = React.useState(false);

  return (
    <div className={`lazy-image-container ${className}`}>
      {!loaded && !error && <Skeleton variant="rectangular" width="100%" height="100%" />}
      {error && (
        <div className="image-error">
          <span className="error-icon">🖼️</span>
          <span className="error-text">Failed to load image</span>
        </div>
      )}
      <img
        src={src}
        alt={alt}
        className={`lazy-image ${loaded ? 'loaded' : ''}`}
        onLoad={() => {
          setLoaded(true);
          onLoad?.();
        }}
        onError={() => {
          setError(true);
          onError?.();
        }}
        loading="lazy"
      />
    </div>
  );
};

interface DataStateProps {
  loading: boolean;
  error?: Error | null;
  empty?: boolean;
  children: React.ReactNode;
  loadingComponent?: React.ReactNode;
  errorComponent?: React.ReactNode;
  emptyComponent?: React.ReactNode;
  onRetry?: () => void;
}

/**
 * Comprehensive data state handler
 */
export const DataState: React.FC<DataStateProps> = ({
  loading,
  error,
  empty,
  children,
  loadingComponent,
  errorComponent,
  emptyComponent,
  onRetry,
}) => {
  if (loading) {
    return <>{loadingComponent || <LoadingOverlay />}</>;
  }

  if (error) {
    return (
      <>
        {errorComponent || (
          <div className="data-error-state">
            <span className="error-icon">⚠️</span>
            <h3>Something went wrong</h3>
            <p>{error.message || 'An unexpected error occurred'}</p>
            {onRetry && (
              <button className="retry-button" onClick={onRetry}>
                Try Again
              </button>
            )}
          </div>
        )}
      </>
    );
  }

  if (empty) {
    return (
      <>
        {emptyComponent || (
          <div className="data-empty-state">
            <span className="empty-icon">📭</span>
            <h3>No data found</h3>
            <p>Try adjusting your filters or search terms</p>
          </div>
        )}
      </>
    );
  }

  return <>{children}</>;
};

/**
 * Progress indicator for multi-step processes
 */
interface StepProgressProps {
  steps: string[];
  currentStep: number;
  variant?: 'linear' | 'circular';
}

export const StepProgress: React.FC<StepProgressProps> = ({
  steps,
  currentStep,
  variant = 'linear',
}) => {
  if (variant === 'circular') {
    const progress = ((currentStep + 1) / steps.length) * 100;
    return (
      <div className="step-progress-circular">
        <svg width="120" height="120" viewBox="0 0 120 120">
          <circle
            cx="60"
            cy="60"
            r="50"
            fill="none"
            stroke="var(--border)"
            strokeWidth="8"
          />
          <circle
            cx="60"
            cy="60"
            r="50"
            fill="none"
            stroke="var(--accent)"
            strokeWidth="8"
            strokeLinecap="round"
            strokeDasharray={`${progress * 3.14} 314`}
            transform="rotate(-90 60 60)"
          />
        </svg>
        <div className="progress-text">
          <span className="progress-step">{currentStep + 1}/{steps.length}</span>
          <span className="progress-label">{steps[currentStep]}</span>
        </div>
      </div>
    );
  }

  return (
    <div className="step-progress-linear">
      {steps.map((step, index) => (
        <div 
          key={index} 
          className={`step ${index <= currentStep ? 'completed' : ''} ${index === currentStep ? 'current' : ''}`}
        >
          <div className="step-indicator">
            {index < currentStep ? '✓' : index + 1}
          </div>
          <span className="step-label">{step}</span>
          {index < steps.length - 1 && <div className="step-connector" />}
        </div>
      ))}
    </div>
  );
};

export default {
  Skeleton,
  DocumentSkeleton,
  SpaceSkeleton,
  ChatMessageSkeleton,
  SearchResultSkeleton,
  Spinner,
  LoadingOverlay,
  LoadingButton,
  LazyImage,
  DataState,
  StepProgress,
};