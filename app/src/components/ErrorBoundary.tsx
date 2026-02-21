import React, { Component, ErrorInfo, ReactNode } from 'react';
import { captureError, addBreadcrumb } from '../lib/errorReporting';
import './ErrorBoundary.css';

interface Props {
  children: ReactNode;
  fallback?: ReactNode;
  onError?: (error: Error, errorInfo: ErrorInfo) => void;
  resetKeys?: Array<string | number>;
  resetOnPropsChange?: boolean;
  isolate?: boolean;
  level?: 'page' | 'section' | 'component';
}

interface State {
  hasError: boolean;
  error: Error | null;
  errorInfo: ErrorInfo | null;
  errorCount: number;
  lastErrorTime: number;
}

/**
 * Production-grade error boundary with recovery mechanisms
 */
class ErrorBoundary extends Component<Props, State> {
  private resetTimeoutId: NodeJS.Timeout | null = null;
  private readonly ERROR_RESET_TIME = 10000; // 10 seconds
  private readonly MAX_ERROR_COUNT = 3;

  constructor(props: Props) {
    super(props);
    this.state = {
      hasError: false,
      error: null,
      errorInfo: null,
      errorCount: 0,
      lastErrorTime: 0,
    };
  }

  static getDerivedStateFromError(error: Error): Partial<State> {
    return {
      hasError: true,
      error,
      lastErrorTime: Date.now(),
    };
  }

  componentDidCatch(error: Error, errorInfo: ErrorInfo) {
    const { onError, level = 'component' } = this.props;
    const { errorCount } = this.state;

    const context = {
      level,
      componentStack: errorInfo.componentStack,
      errorCount: errorCount + 1,
      timestamp: new Date().toISOString(),
    };

    console.error('React Error Boundary Caught Error', context);

    captureError(error, context);
    addBreadcrumb('Error boundary triggered', 'error', context);

    // Update state with error details
    this.setState(prevState => ({
      errorInfo,
      errorCount: prevState.errorCount + 1,
    }));

    // Call custom error handler if provided
    if (onError) {
      onError(error, errorInfo);
    }

    // Send error to telemetry service in production
    if (import.meta.env.PROD) {
      this.reportErrorToService(error, errorInfo);
    }

    // Auto-recovery mechanism
    this.setupAutoRecovery();
  }

  componentDidUpdate(prevProps: Props) {
    const { resetKeys, resetOnPropsChange } = this.props;
    const { hasError } = this.state;

    if (hasError && prevProps.resetKeys !== resetKeys) {
      if (resetKeys?.some((key, idx) => key !== prevProps.resetKeys?.[idx])) {
        this.resetErrorBoundary();
      }
    }

    if (hasError && resetOnPropsChange && prevProps.children !== this.props.children) {
      this.resetErrorBoundary();
    }
  }

  componentWillUnmount() {
    if (this.resetTimeoutId) {
      clearTimeout(this.resetTimeoutId);
    }
  }

  setupAutoRecovery = () => {
    const { errorCount } = this.state;

    // Don't auto-recover if too many errors
    if (errorCount >= this.MAX_ERROR_COUNT) {
      console.warn('Max error count reached, disabling auto-recovery');
      addBreadcrumb('Max error count reached', 'recovery', { errorCount });
      return;
    }

    // Clear any existing timeout
    if (this.resetTimeoutId) {
      clearTimeout(this.resetTimeoutId);
    }

    // Set up auto-recovery after delay
    this.resetTimeoutId = setTimeout(() => {
      console.log('Attempting auto-recovery from error');
      addBreadcrumb('Auto-recovery attempted', 'recovery', {});
      this.resetErrorBoundary();
    }, this.ERROR_RESET_TIME);
  };

  resetErrorBoundary = () => {
    const { errorCount, lastErrorTime } = this.state;
    
    // Reset error count if enough time has passed
    const shouldResetCount = Date.now() - lastErrorTime > 60000; // 1 minute
    
    this.setState({
      hasError: false,
      error: null,
      errorInfo: null,
      errorCount: shouldResetCount ? 0 : errorCount,
    });

    if (this.resetTimeoutId) {
      clearTimeout(this.resetTimeoutId);
      this.resetTimeoutId = null;
    }
  };

  reportErrorToService = async (error: Error, errorInfo: ErrorInfo) => {
    try {
      const errorReport = {
        message: error.message,
        stack: error.stack,
        componentStack: errorInfo.componentStack,
        timestamp: new Date().toISOString(),
        userAgent: navigator.userAgent,
        url: window.location.href,
        userId: localStorage.getItem('userId'),
        sessionId: sessionStorage.getItem('sessionId'),
      };

      captureError(error, errorReport);
      console.log('Error reported to service', errorReport);
    } catch (reportError) {
      console.error('Failed to report error to service', reportError);
    }
  };

  render() {
    const { hasError, error, errorInfo, errorCount } = this.state;
    const { children, fallback, isolate, level = 'component' } = this.props;

    if (hasError) {
      // Custom fallback UI
      if (fallback) {
        return <>{fallback}</>;
      }

      // Too many errors - show permanent error state
      if (errorCount >= this.MAX_ERROR_COUNT) {
        return (
          <div className={`error-boundary-fallback error-level-${level} error-critical`}>
            <div className="error-icon">⚠️</div>
            <h2>Critical Error</h2>
            <p>This component has encountered multiple errors and cannot recover automatically.</p>
            <button 
              className="error-refresh-btn"
              onClick={() => window.location.reload()}
            >
              Refresh Page
            </button>
          </div>
        );
      }

      // Default error UI
      return (
        <div className={`error-boundary-fallback error-level-${level}`}>
          <div className="error-icon">😕</div>
          <h2>Something went wrong</h2>
          <p className="error-message">
            {error?.message || 'An unexpected error occurred'}
          </p>
          
          {process.env.NODE_ENV === 'development' && (
            <details className="error-details">
              <summary>Error Details (Development Only)</summary>
              <pre>{error?.stack}</pre>
              <pre>{errorInfo?.componentStack}</pre>
            </details>
          )}
          
          <div className="error-actions">
            <button 
              className="error-retry-btn"
              onClick={this.resetErrorBoundary}
            >
              Try Again
            </button>
            
            {!isolate && (
              <button 
                className="error-refresh-btn"
                onClick={() => window.location.reload()}
              >
                Refresh Page
              </button>
            )}
          </div>
          
          {errorCount > 1 && (
            <p className="error-count">
              Error occurred {errorCount} times
            </p>
          )}
        </div>
      );
    }

    return children;
  }
}

/**
 * Async error boundary for handling promise rejections
 */
export const AsyncErrorBoundary: React.FC<Props> = ({ children, ...props }) => {
  React.useEffect(() => {
    const handleUnhandledRejection = (event: PromiseRejectionEvent) => {
      const error = event.reason instanceof Error ? event.reason : new Error(String(event.reason));

      console.error('Unhandled Promise Rejection', {
        reason: event.reason,
        promise: event.promise,
      });

      captureError(error, {
        type: 'unhandledRejection',
        reason: event.reason,
      });

      addBreadcrumb('Unhandled promise rejection', 'error', {
        reason: String(event.reason),
      });
    };

    window.addEventListener('unhandledrejection', handleUnhandledRejection);

    return () => {
      window.removeEventListener('unhandledrejection', handleUnhandledRejection);
    };
  }, []);

  return <ErrorBoundary {...props}>{children}</ErrorBoundary>;
};

/**
 * Hook for imperatively throwing errors to nearest error boundary
 */
export const useErrorHandler = () => {
  return React.useCallback((error: Error) => {
    throw error;
  }, []);
};

export default ErrorBoundary;