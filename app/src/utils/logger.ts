/**
 * Production-grade logging system with multiple transports and log levels
 */

export enum LogLevel {
  DEBUG = 0,
  INFO = 1,
  WARN = 2,
  ERROR = 3,
  CRITICAL = 4,
}

export interface LogEntry {
  level: LogLevel;
  message: string;
  timestamp: string;
  context?: Record<string, any>;
  stack?: string;
  userId?: string;
  sessionId?: string;
  environment?: string;
}

interface LoggerConfig {
  level: LogLevel;
  enableConsole: boolean;
  enableRemote: boolean;
  enableLocalStorage: boolean;
  maxStoredLogs: number;
  remoteEndpoint?: string;
  batchSize: number;
  flushInterval: number;
}

class Logger {
  private config: LoggerConfig;
  private logBuffer: LogEntry[] = [];
  private flushTimer: NodeJS.Timeout | null = null;
  private sessionId: string;

  constructor(config?: Partial<LoggerConfig>) {
    this.config = {
      level: process.env.NODE_ENV === 'production' ? LogLevel.INFO : LogLevel.DEBUG,
      enableConsole: process.env.NODE_ENV !== 'production',
      enableRemote: process.env.NODE_ENV === 'production',
      enableLocalStorage: true,
      maxStoredLogs: 1000,
      batchSize: 50,
      flushInterval: 5000,
      ...config,
    };

    this.sessionId = this.generateSessionId();
    this.setupFlushTimer();
    this.setupErrorHandlers();
  }

  private generateSessionId(): string {
    return `${Date.now()}-${Math.random().toString(36).substr(2, 9)}`;
  }

  private setupFlushTimer(): void {
    if (this.config.enableRemote && this.config.flushInterval > 0) {
      this.flushTimer = setInterval(() => {
        this.flush();
      }, this.config.flushInterval);
    }
  }

  private setupErrorHandlers(): void {
    // Capture unhandled errors
    window.addEventListener('error', (event) => {
      this.error('Unhandled Error', {
        message: event.message,
        filename: event.filename,
        lineno: event.lineno,
        colno: event.colno,
        error: event.error?.stack,
      });
    });

    // Capture unhandled promise rejections
    window.addEventListener('unhandledrejection', (event) => {
      this.error('Unhandled Promise Rejection', {
        reason: event.reason,
        promise: event.promise,
      });
    });
  }

  private shouldLog(level: LogLevel): boolean {
    return level >= this.config.level;
  }

  private formatMessage(level: LogLevel, message: string, context?: Record<string, any>): LogEntry {
    return {
      level,
      message,
      timestamp: new Date().toISOString(),
      context,
      userId: localStorage.getItem('userId') || undefined,
      sessionId: this.sessionId,
      environment: process.env.NODE_ENV,
    };
  }

  private consoleLog(entry: LogEntry): void {
    if (!this.config.enableConsole) return;

    const style = this.getConsoleStyle(entry.level);
    const levelName = LogLevel[entry.level];
    const prefix = `%c[${levelName}] ${entry.timestamp}`;

    if (entry.context) {
      console.groupCollapsed(prefix, style, entry.message);
      console.log('Context:', entry.context);
      if (entry.stack) {
        console.log('Stack:', entry.stack);
      }
      console.groupEnd();
    } else {
      console.log(prefix, style, entry.message);
    }
  }

  private getConsoleStyle(level: LogLevel): string {
    const styles = {
      [LogLevel.DEBUG]: 'color: #gray; font-weight: normal;',
      [LogLevel.INFO]: 'color: #2563eb; font-weight: normal;',
      [LogLevel.WARN]: 'color: #f59e0b; font-weight: bold;',
      [LogLevel.ERROR]: 'color: #ef4444; font-weight: bold;',
      [LogLevel.CRITICAL]: 'color: #dc2626; font-weight: bold; font-size: 14px;',
    };
    return styles[level] || '';
  }

  private storeLog(entry: LogEntry): void {
    if (!this.config.enableLocalStorage) return;

    try {
      const storedLogs = this.getStoredLogs();
      storedLogs.push(entry);

      // Keep only the most recent logs
      if (storedLogs.length > this.config.maxStoredLogs) {
        storedLogs.splice(0, storedLogs.length - this.config.maxStoredLogs);
      }

      localStorage.setItem('app_logs', JSON.stringify(storedLogs));
    } catch (error) {
      // Handle localStorage quota exceeded
      if (error instanceof DOMException && error.code === 22) {
        this.clearStoredLogs();
      }
    }
  }

  private getStoredLogs(): LogEntry[] {
    try {
      const logs = localStorage.getItem('app_logs');
      return logs ? JSON.parse(logs) : [];
    } catch {
      return [];
    }
  }

  private clearStoredLogs(): void {
    localStorage.removeItem('app_logs');
  }

  private addToBuffer(entry: LogEntry): void {
    if (!this.config.enableRemote) return;

    this.logBuffer.push(entry);

    if (this.logBuffer.length >= this.config.batchSize) {
      this.flush();
    }
  }

  private async flush(): Promise<void> {
    if (this.logBuffer.length === 0 || !this.config.remoteEndpoint) return;

    const logsToSend = [...this.logBuffer];
    this.logBuffer = [];

    try {
      await fetch(this.config.remoteEndpoint, {
        method: 'POST',
        headers: {
          'Content-Type': 'application/json',
          'X-Session-Id': this.sessionId,
        },
        body: JSON.stringify({ logs: logsToSend }),
      });
    } catch (error) {
      // Re-add logs to buffer on failure
      this.logBuffer.unshift(...logsToSend);
      console.error('Failed to send logs to remote server:', error);
    }
  }

  private log(level: LogLevel, message: string, context?: Record<string, any>): void {
    if (!this.shouldLog(level)) return;

    const entry = this.formatMessage(level, message, context);

    // Add stack trace for errors
    if (level >= LogLevel.ERROR) {
      entry.stack = new Error().stack;
    }

    this.consoleLog(entry);
    this.storeLog(entry);
    this.addToBuffer(entry);
  }

  // Public logging methods
  public debug(message: string, context?: Record<string, any>): void {
    this.log(LogLevel.DEBUG, message, context);
  }

  public info(message: string, context?: Record<string, any>): void {
    this.log(LogLevel.INFO, message, context);
  }

  public warn(message: string, context?: Record<string, any>): void {
    this.log(LogLevel.WARN, message, context);
  }

  public error(message: string, context?: Record<string, any>): void {
    this.log(LogLevel.ERROR, message, context);
  }

  public critical(message: string, context?: Record<string, any>): void {
    this.log(LogLevel.CRITICAL, message, context);
    // Immediately flush critical logs
    this.flush();
  }

  // Performance logging
  public time(label: string): void {
    if (this.config.enableConsole) {
      console.time(label);
    }
  }

  public timeEnd(label: string): void {
    if (this.config.enableConsole) {
      console.timeEnd(label);
    }
  }

  // Group logging
  public group(label: string): void {
    if (this.config.enableConsole) {
      console.group(label);
    }
  }

  public groupEnd(): void {
    if (this.config.enableConsole) {
      console.groupEnd();
    }
  }

  // Table logging
  public table(data: any): void {
    if (this.config.enableConsole) {
      console.table(data);
    }
  }

  // Get logs for debugging
  public getLogs(level?: LogLevel): LogEntry[] {
    const logs = this.getStoredLogs();
    if (level !== undefined) {
      return logs.filter(log => log.level >= level);
    }
    return logs;
  }

  // Export logs for support
  public exportLogs(): string {
    const logs = this.getStoredLogs();
    return JSON.stringify(logs, null, 2);
  }

  // Download logs as file
  public downloadLogs(): void {
    const logs = this.exportLogs();
    const blob = new Blob([logs], { type: 'application/json' });
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = `app-logs-${this.sessionId}.json`;
    a.click();
    URL.revokeObjectURL(url);
  }

  // Clear all logs
  public clear(): void {
    this.clearStoredLogs();
    this.logBuffer = [];
    if (this.config.enableConsole) {
      console.clear();
    }
  }

  // Update configuration
  public setConfig(config: Partial<LoggerConfig>): void {
    this.config = { ...this.config, ...config };
    
    // Restart flush timer if needed
    if (this.flushTimer) {
      clearInterval(this.flushTimer);
      this.setupFlushTimer();
    }
  }

  // Cleanup
  public destroy(): void {
    if (this.flushTimer) {
      clearInterval(this.flushTimer);
    }
    this.flush();
  }
}

// Create singleton instance
export const logger = new Logger({
  remoteEndpoint: process.env.REACT_APP_LOG_ENDPOINT,
});

// Performance monitoring utilities
export const performance = {
  mark(name: string): void {
    if (window.performance && window.performance.mark) {
      window.performance.mark(name);
    }
  },

  measure(name: string, startMark: string, endMark?: string): void {
    if (window.performance && window.performance.measure) {
      try {
        if (endMark) {
          window.performance.measure(name, startMark, endMark);
        } else {
          window.performance.measure(name, startMark);
        }
        
        const entries = window.performance.getEntriesByName(name, 'measure');
        if (entries.length > 0) {
          const duration = entries[entries.length - 1].duration;
          logger.debug(`Performance: ${name}`, { duration: `${duration.toFixed(2)}ms` });
        }
      } catch (error) {
        logger.error('Performance measurement failed', { name, error });
      }
    }
  },

  clearMarks(): void {
    if (window.performance && window.performance.clearMarks) {
      window.performance.clearMarks();
    }
  },

  clearMeasures(): void {
    if (window.performance && window.performance.clearMeasures) {
      window.performance.clearMeasures();
    }
  },
};

export default logger;