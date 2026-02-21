import * as Sentry from '@sentry/react';

const IS_PRODUCTION = import.meta.env.PROD;
const SENTRY_DSN = import.meta.env.VITE_SENTRY_DSN;

export function initErrorReporting() {
  if (!IS_PRODUCTION || !SENTRY_DSN) {
    console.log('Error reporting disabled (development mode)');
    return;
  }

  Sentry.init({
    dsn: SENTRY_DSN,
    environment: IS_PRODUCTION ? 'production' : 'development',
    integrations: [
      Sentry.browserTracingIntegration(),
      Sentry.replayIntegration({
        maskAllText: true,
        blockAllMedia: true,
      }),
    ],
    tracesSampleRate: 1.0,
    replaysSessionSampleRate: 0.1,
    replaysOnErrorSampleRate: 1.0,
  });
}

export function captureError(error: Error, context?: Record<string, any>) {
  console.error('Error:', error, context);

  if (IS_PRODUCTION && SENTRY_DSN) {
    Sentry.captureException(error, {
      extra: context,
    });
  }
}

export function setUserContext(userId: string, email?: string) {
  if (IS_PRODUCTION && SENTRY_DSN) {
    Sentry.setUser({
      id: userId,
      email,
    });
  }
}

export function addBreadcrumb(message: string, category: string, data?: Record<string, any>) {
  if (IS_PRODUCTION && SENTRY_DSN) {
    Sentry.addBreadcrumb({
      message,
      category,
      data,
      level: 'info',
    });
  }
}
