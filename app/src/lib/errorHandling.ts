import { toast } from 'sonner';
import { captureError, addBreadcrumb } from './errorReporting';

export interface ErrorHandlingOptions {
  silent?: boolean;
  userMessage?: string;
  retryable?: boolean;
  onRetry?: () => Promise<void>;
  context?: Record<string, any>;
}

export class AppError extends Error {
  constructor(
    message: string,
    public userMessage: string,
    public retryable: boolean = false,
    public context?: Record<string, any>
  ) {
    super(message);
    this.name = 'AppError';
  }
}

export async function handleAsyncOperation<T>(
  operation: () => Promise<T>,
  options: ErrorHandlingOptions = {}
): Promise<T | null> {
  const {
    silent = false,
    userMessage = 'An error occurred',
    retryable = false,
    onRetry,
    context = {},
  } = options;

  try {
    addBreadcrumb('Async operation started', 'operation', context);
    const result = await operation();
    addBreadcrumb('Async operation completed', 'operation', context);
    return result;
  } catch (error) {
    const err = error as Error;

    addBreadcrumb('Async operation failed', 'error', {
      ...context,
      error: err.message,
    });

    captureError(err, context);

    if (!silent) {
      if (retryable && onRetry) {
        toast.error(userMessage, {
          action: {
            label: 'Retry',
            onClick: async () => {
              try {
                await onRetry();
                toast.success('Operation successful');
              } catch (retryError) {
                toast.error('Retry failed');
                captureError(retryError as Error, { ...context, isRetry: true });
              }
            },
          },
        });
      } else {
        toast.error(userMessage);
      }
    }

    return null;
  }
}

export function getErrorMessage(error: unknown): string {
  if (error instanceof AppError) {
    return error.userMessage;
  }

  if (error instanceof Error) {
    return error.message;
  }

  if (typeof error === 'string') {
    return error;
  }

  return 'An unexpected error occurred';
}

export function handleInvokeError(error: unknown, operation: string): string {
  const message = getErrorMessage(error);

  const errorMap: Record<string, string> = {
    'Failed to index': 'Could not index the workspace. Please check the folder path.',
    'Permission denied': 'Permission denied. Please run as administrator or check folder permissions.',
    'No space left': 'Not enough disk space to complete the operation.',
    'Database error': 'Database error. Try restarting the application.',
    'LLM not configured': 'Please configure an LLM provider in Settings.',
  };

  for (const [key, userMessage] of Object.entries(errorMap)) {
    if (message.toLowerCase().includes(key.toLowerCase())) {
      return userMessage;
    }
  }

  return `Failed to ${operation}: ${message}`;
}
