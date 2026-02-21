import { toast } from 'sonner';
import { captureError, addBreadcrumb } from './errorReporting';

export interface RetryOptions {
  maxAttempts?: number;
  delayMs?: number;
  backoffMultiplier?: number;
  onRetry?: (attempt: number, error: Error) => void;
  shouldRetry?: (error: Error) => boolean;
  operationName?: string;
  showToast?: boolean;
}

const DEFAULT_OPTIONS: Required<Omit<RetryOptions, 'onRetry' | 'shouldRetry' | 'operationName'>> = {
  maxAttempts: 3,
  delayMs: 1000,
  backoffMultiplier: 2,
  showToast: true,
};

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function isRetryableError(error: Error): boolean {
  const errorMessage = error.message.toLowerCase();

  const retryablePatterns = [
    'network',
    'timeout',
    'econnrefused',
    'enotfound',
    'etimedout',
    'temporary',
    'unavailable',
    'busy',
    'locked',
  ];

  return retryablePatterns.some((pattern) => errorMessage.includes(pattern));
}

export async function withRetry<T>(
  operation: () => Promise<T>,
  options: RetryOptions = {}
): Promise<T> {
  const {
    maxAttempts,
    delayMs,
    backoffMultiplier,
    onRetry,
    shouldRetry,
    operationName,
    showToast,
  } = { ...DEFAULT_OPTIONS, ...options };

  let lastError: Error | undefined;
  let currentDelay = delayMs;

  for (let attempt = 1; attempt <= maxAttempts; attempt++) {
    try {
      addBreadcrumb(
        `Attempt ${attempt}/${maxAttempts}`,
        'retry',
        { operationName }
      );

      const result = await operation();

      if (attempt > 1 && showToast) {
        toast.success(`${operationName || 'Operation'} succeeded after ${attempt} attempts`);
      }

      return result;
    } catch (error) {
      lastError = error as Error;

      const isLastAttempt = attempt === maxAttempts;
      const retry = shouldRetry ? shouldRetry(lastError) : isRetryableError(lastError);

      if (!retry || isLastAttempt) {
        addBreadcrumb(
          'Retry failed',
          'error',
          { operationName, attempts: attempt, error: lastError.message }
        );

        captureError(lastError, {
          context: 'retry_exhausted',
          operationName,
          attempts: attempt,
        });

        if (showToast) {
          toast.error(
            `${operationName || 'Operation'} failed after ${attempt} attempts: ${lastError.message}`
          );
        }

        throw lastError;
      }

      addBreadcrumb(
        `Retry attempt ${attempt} failed`,
        'warning',
        { operationName, error: lastError.message, nextRetryIn: currentDelay }
      );

      if (onRetry) {
        onRetry(attempt, lastError);
      }

      if (showToast && attempt < maxAttempts) {
        toast.warning(
          `${operationName || 'Operation'} failed. Retrying (${attempt}/${maxAttempts})...`,
          {
            duration: currentDelay,
          }
        );
      }

      await sleep(currentDelay);
      currentDelay *= backoffMultiplier;
    }
  }

  throw lastError || new Error('Operation failed with no error');
}

export class RetryableOperation<T> {
  private options: RetryOptions;
  private operation: () => Promise<T>;

  constructor(operation: () => Promise<T>, options: RetryOptions = {}) {
    this.operation = operation;
    this.options = options;
  }

  async execute(): Promise<T> {
    return withRetry(this.operation, this.options);
  }

  withMaxAttempts(maxAttempts: number): this {
    this.options.maxAttempts = maxAttempts;
    return this;
  }

  withDelay(delayMs: number): this {
    this.options.delayMs = delayMs;
    return this;
  }

  withBackoff(multiplier: number): this {
    this.options.backoffMultiplier = multiplier;
    return this;
  }

  onRetry(callback: (attempt: number, error: Error) => void): this {
    this.options.onRetry = callback;
    return this;
  }

  shouldRetry(predicate: (error: Error) => boolean): this {
    this.options.shouldRetry = predicate;
    return this;
  }

  named(operationName: string): this {
    this.options.operationName = operationName;
    return this;
  }

  silent(): this {
    this.options.showToast = false;
    return this;
  }
}

export function retry<T>(operation: () => Promise<T>): RetryableOperation<T> {
  return new RetryableOperation(operation);
}
