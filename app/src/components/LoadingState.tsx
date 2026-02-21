import React from 'react';
import { useTheme } from '../contexts/ThemeContext';
import { Loader2 } from 'lucide-react';

interface LoadingStateProps {
  message?: string;
  subMessage?: string;
  progress?: number;
  currentItem?: string;
  processedCount?: number;
  totalCount?: number;
  size?: 'sm' | 'md' | 'lg';
  fullScreen?: boolean;
}

export function LoadingState({
  message = 'Loading...',
  subMessage,
  progress,
  currentItem,
  processedCount,
  totalCount,
  size = 'md',
  fullScreen = false,
}: LoadingStateProps) {
  const { colors } = useTheme();

  const sizeClasses = {
    sm: 'w-4 h-4',
    md: 'w-8 h-8',
    lg: 'w-12 h-12',
  };

  const textSizes = {
    sm: 'text-sm',
    md: 'text-base',
    lg: 'text-lg',
  };

  const content = (
    <div className="flex flex-col items-center justify-center gap-4 p-6">
      <Loader2 className={`${sizeClasses[size]} animate-spin`} style={{ color: colors.primary }} />

      <div className="text-center max-w-md">
        <p className={`${textSizes[size]} font-medium mb-1`} style={{ color: colors.text }}>
          {message}
        </p>

        {subMessage && (
          <p className="text-sm" style={{ color: colors.textMuted }}>
            {subMessage}
          </p>
        )}

        {currentItem && (
          <p className="text-xs mt-2 font-mono truncate max-w-md" style={{ color: colors.textSecondary }}>
            {currentItem}
          </p>
        )}

        {(processedCount !== undefined && totalCount !== undefined) && (
          <p className="text-sm mt-2" style={{ color: colors.textMuted }}>
            {processedCount} / {totalCount} items
          </p>
        )}
      </div>

      {progress !== undefined && (
        <div className="w-full max-w-md">
          <div className="h-2 rounded-full overflow-hidden" style={{ background: colors.bgTertiary }}>
            <div
              className="h-full transition-all duration-300"
              style={{
                background: colors.primary,
                width: `${progress}%`,
              }}
            />
          </div>
          <p className="text-xs text-center mt-2" style={{ color: colors.textMuted }}>
            {Math.round(progress)}%
          </p>
        </div>
      )}
    </div>
  );

  if (fullScreen) {
    return (
      <div className="fixed inset-0 z-50 flex items-center justify-center" style={{ background: colors.bg }}>
        {content}
      </div>
    );
  }

  return content;
}

export function SkeletonLoader({ className = '' }: { className?: string }) {
  const { colors } = useTheme();

  return (
    <div
      className={`animate-pulse rounded ${className}`}
      style={{ background: colors.bgTertiary }}
    />
  );
}

export function SkeletonText({ lines = 3, className = '' }: { lines?: number; className?: string }) {
  return (
    <div className={`space-y-2 ${className}`}>
      {Array.from({ length: lines }).map((_, i) => (
        <SkeletonLoader
          key={i}
          className={`h-4 ${i === lines - 1 ? 'w-2/3' : 'w-full'}`}
        />
      ))}
    </div>
  );
}

export function SkeletonCard({ className = '' }: { className?: string }) {
  const { colors } = useTheme();

  return (
    <div
      className={`rounded-lg border-2 p-6 ${className}`}
      style={{ background: colors.bgSecondary, borderColor: colors.border }}
    >
      <div className="flex items-start gap-4">
        <SkeletonLoader className="w-12 h-12 rounded-full flex-shrink-0" />
        <div className="flex-1 space-y-3">
          <SkeletonLoader className="h-6 w-1/3" />
          <SkeletonText lines={2} />
        </div>
      </div>
    </div>
  );
}

export function SkeletonList({ count = 5, className = '' }: { count?: number; className?: string }) {
  return (
    <div className={`space-y-4 ${className}`}>
      {Array.from({ length: count }).map((_, i) => (
        <SkeletonCard key={i} />
      ))}
    </div>
  );
}
