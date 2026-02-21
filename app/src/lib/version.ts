export const APP_VERSION = import.meta.env.VITE_APP_VERSION || '0.1.0-beta';
export const APP_BUILD_DATE = import.meta.env.VITE_APP_BUILD_DATE || new Date().toISOString();
export const IS_BETA = APP_VERSION.includes('beta') || APP_VERSION.includes('alpha');
export const IS_DEV = import.meta.env.DEV;
export const IS_PROD = import.meta.env.PROD;

export function getAppInfo() {
  return {
    version: APP_VERSION,
    buildDate: APP_BUILD_DATE,
    isBeta: IS_BETA,
    isDevelopment: IS_DEV,
    isProduction: IS_PROD,
    userAgent: navigator.userAgent,
    platform: navigator.platform,
  };
}

export function getVersionDisplay(): string {
  if (IS_BETA) {
    return `v${APP_VERSION} (Beta)`;
  }
  return `v${APP_VERSION}`;
}
