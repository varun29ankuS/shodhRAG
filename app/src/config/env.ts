/**
 * Production-grade environment configuration
 * Centralizes all environment variables and provides type-safe access
 */

import { logger } from '../utils/logger';

export enum Environment {
  DEVELOPMENT = 'development',
  STAGING = 'staging',
  PRODUCTION = 'production',
  TEST = 'test',
}

interface EnvConfig {
  // App Configuration
  APP_NAME: string;
  APP_VERSION: string;
  ENVIRONMENT: Environment;
  DEBUG: boolean;
  
  // API Configuration
  API_BASE_URL: string;
  API_TIMEOUT: number;
  API_RETRY_ATTEMPTS: number;
  API_RETRY_DELAY: number;
  
  // Authentication
  AUTH_ENABLED: boolean;
  AUTH_PROVIDER: 'local' | 'oauth' | 'saml';
  SESSION_DURATION: number;
  REFRESH_TOKEN_DURATION: number;
  
  // LLM Configuration
  LLM_DEFAULT_MODE: string;
  LLM_MAX_TOKENS: number;
  LLM_TEMPERATURE: number;
  LLM_MODEL_PATH?: string;
  
  // Storage Configuration
  STORAGE_TYPE: 'local' | 'cloud';
  STORAGE_PATH: string;
  MAX_FILE_SIZE: number;
  ALLOWED_FILE_TYPES: string[];
  
  // Feature Flags
  FEATURES: {
    KNOWLEDGE_GRAPH: boolean;
    CHAT_HISTORY: boolean;
    SEARCH_SUGGESTIONS: boolean;
    FILE_WATCHING: boolean;
    EXPORT_IMPORT: boolean;
    ANALYTICS: boolean;
    MULTI_USER: boolean;
  };
  
  // Performance
  CACHE_ENABLED: boolean;
  CACHE_SIZE: number;
  CACHE_TTL: number;
  MAX_CONCURRENT_OPERATIONS: number;
  
  // Security
  ENCRYPTION_ENABLED: boolean;
  ENCRYPTION_KEY?: string;
  CSP_ENABLED: boolean;
  CORS_ORIGINS: string[];
  
  // Monitoring
  TELEMETRY_ENABLED: boolean;
  TELEMETRY_ENDPOINT?: string;
  LOG_LEVEL: 'debug' | 'info' | 'warn' | 'error';
  LOG_ENDPOINT?: string;
  SENTRY_DSN?: string;
  
  // UI Configuration
  DEFAULT_THEME: 'light' | 'dark';
  DEFAULT_LANGUAGE: string;
  ANIMATIONS_ENABLED: boolean;
  
  // Rate Limiting
  RATE_LIMIT_ENABLED: boolean;
  RATE_LIMIT_WINDOW: number;
  RATE_LIMIT_MAX_REQUESTS: number;
}

class Config {
  private config: EnvConfig;
  private readonly defaults: EnvConfig = {
    // App Configuration
    APP_NAME: 'Kalki RAG',
    APP_VERSION: '2.0.0',
    ENVIRONMENT: Environment.PRODUCTION,
    DEBUG: false,
    
    // API Configuration
    API_BASE_URL: 'http://localhost:3000',
    API_TIMEOUT: 30000,
    API_RETRY_ATTEMPTS: 3,
    API_RETRY_DELAY: 1000,
    
    // Authentication
    AUTH_ENABLED: false,
    AUTH_PROVIDER: 'local',
    SESSION_DURATION: 3600000, // 1 hour
    REFRESH_TOKEN_DURATION: 604800000, // 7 days
    
    // LLM Configuration
    LLM_DEFAULT_MODE: 'disabled',
    LLM_MAX_TOKENS: 2048,
    LLM_TEMPERATURE: 0.7,
    
    // Storage Configuration
    STORAGE_TYPE: 'local',
    STORAGE_PATH: './data',
    MAX_FILE_SIZE: 10485760, // 10MB
    ALLOWED_FILE_TYPES: ['pdf', 'txt', 'md', 'docx', 'csv'],
    
    // Feature Flags
    FEATURES: {
      KNOWLEDGE_GRAPH: true,
      CHAT_HISTORY: true,
      SEARCH_SUGGESTIONS: true,
      FILE_WATCHING: true,
      EXPORT_IMPORT: true,
      ANALYTICS: false,
      MULTI_USER: false,
    },
    
    // Performance
    CACHE_ENABLED: true,
    CACHE_SIZE: 52428800, // 50MB
    CACHE_TTL: 3600000, // 1 hour
    MAX_CONCURRENT_OPERATIONS: 5,
    
    // Security
    ENCRYPTION_ENABLED: false,
    CSP_ENABLED: true,
    CORS_ORIGINS: ['http://localhost:3000'],
    
    // Monitoring
    TELEMETRY_ENABLED: false,
    LOG_LEVEL: 'info',
    
    // UI Configuration
    DEFAULT_THEME: 'dark',
    DEFAULT_LANGUAGE: 'en',
    ANIMATIONS_ENABLED: true,
    
    // Rate Limiting
    RATE_LIMIT_ENABLED: true,
    RATE_LIMIT_WINDOW: 60000, // 1 minute
    RATE_LIMIT_MAX_REQUESTS: 100,
  };
  
  constructor() {
    this.config = this.loadConfig();
    this.validateConfig();
    this.logConfig();
  }
  
  private loadConfig(): EnvConfig {
    const config = { ...this.defaults };
    
    // Load from environment variables
    if (typeof process !== 'undefined' && process.env) {
      // App Configuration
      config.APP_NAME = process.env.REACT_APP_NAME || config.APP_NAME;
      config.APP_VERSION = process.env.REACT_APP_VERSION || config.APP_VERSION;
      config.ENVIRONMENT = (process.env.NODE_ENV as Environment) || config.ENVIRONMENT;
      config.DEBUG = process.env.REACT_APP_DEBUG === 'true';
      
      // API Configuration
      config.API_BASE_URL = process.env.REACT_APP_API_BASE_URL || config.API_BASE_URL;
      config.API_TIMEOUT = parseInt(process.env.REACT_APP_API_TIMEOUT || '') || config.API_TIMEOUT;
      
      // LLM Configuration
      config.LLM_DEFAULT_MODE = process.env.REACT_APP_LLM_MODE || config.LLM_DEFAULT_MODE;
      config.LLM_MODEL_PATH = process.env.REACT_APP_LLM_MODEL_PATH;
      
      // Feature Flags
      if (process.env.REACT_APP_FEATURES) {
        try {
          const features = JSON.parse(process.env.REACT_APP_FEATURES);
          config.FEATURES = { ...config.FEATURES, ...features };
        } catch (error) {
          logger.error('Failed to parse feature flags', { error });
        }
      }
      
      // Security
      config.ENCRYPTION_KEY = process.env.REACT_APP_ENCRYPTION_KEY;
      
      // Monitoring
      config.TELEMETRY_ENABLED = process.env.REACT_APP_TELEMETRY === 'true';
      config.TELEMETRY_ENDPOINT = process.env.REACT_APP_TELEMETRY_ENDPOINT;
      config.LOG_ENDPOINT = process.env.REACT_APP_LOG_ENDPOINT;
      config.SENTRY_DSN = process.env.REACT_APP_SENTRY_DSN;
    }
    
    // Load from localStorage for runtime configuration
    try {
      const savedConfig = localStorage.getItem('app_config');
      if (savedConfig) {
        const parsed = JSON.parse(savedConfig);
        Object.assign(config, parsed);
      }
    } catch (error) {
      logger.error('Failed to load config from localStorage', { error });
    }
    
    return config;
  }
  
  private validateConfig(): void {
    const errors: string[] = [];
    
    // Validate required fields
    if (!this.config.API_BASE_URL) {
      errors.push('API_BASE_URL is required');
    }
    
    // Validate URLs
    try {
      new URL(this.config.API_BASE_URL);
    } catch {
      errors.push('Invalid API_BASE_URL');
    }
    
    // Validate numeric ranges
    if (this.config.API_TIMEOUT < 1000 || this.config.API_TIMEOUT > 300000) {
      errors.push('API_TIMEOUT must be between 1000ms and 300000ms');
    }
    
    if (this.config.LLM_TEMPERATURE < 0 || this.config.LLM_TEMPERATURE > 2) {
      errors.push('LLM_TEMPERATURE must be between 0 and 2');
    }
    
    // Log validation errors
    if (errors.length > 0) {
      logger.error('Configuration validation failed', { errors });
      if (this.config.ENVIRONMENT === Environment.PRODUCTION) {
        throw new Error('Invalid configuration');
      }
    }
  }
  
  private logConfig(): void {
    if (this.config.DEBUG) {
      logger.info('Configuration loaded', {
        environment: this.config.ENVIRONMENT,
        features: this.config.FEATURES,
        api: {
          baseUrl: this.config.API_BASE_URL,
          timeout: this.config.API_TIMEOUT,
        },
      });
    }
  }
  
  // Getters for type-safe access
  get<K extends keyof EnvConfig>(key: K): EnvConfig[K] {
    return this.config[key];
  }
  
  set<K extends keyof EnvConfig>(key: K, value: EnvConfig[K]): void {
    this.config[key] = value;
    this.saveConfig();
  }
  
  // Check if a feature is enabled
  isFeatureEnabled(feature: keyof EnvConfig['FEATURES']): boolean {
    return this.config.FEATURES[feature];
  }
  
  // Environment checks
  isDevelopment(): boolean {
    return this.config.ENVIRONMENT === Environment.DEVELOPMENT;
  }
  
  isStaging(): boolean {
    return this.config.ENVIRONMENT === Environment.STAGING;
  }
  
  isProduction(): boolean {
    return this.config.ENVIRONMENT === Environment.PRODUCTION;
  }
  
  isTest(): boolean {
    return this.config.ENVIRONMENT === Environment.TEST;
  }
  
  // Save configuration to localStorage
  private saveConfig(): void {
    try {
      // Only save user-modifiable settings
      const toSave = {
        DEFAULT_THEME: this.config.DEFAULT_THEME,
        DEFAULT_LANGUAGE: this.config.DEFAULT_LANGUAGE,
        ANIMATIONS_ENABLED: this.config.ANIMATIONS_ENABLED,
        LOG_LEVEL: this.config.LOG_LEVEL,
        FEATURES: this.config.FEATURES,
      };
      localStorage.setItem('app_config', JSON.stringify(toSave));
    } catch (error) {
      logger.error('Failed to save config', { error });
    }
  }
  
  // Reset to defaults
  reset(): void {
    this.config = { ...this.defaults };
    localStorage.removeItem('app_config');
    logger.info('Configuration reset to defaults');
  }
  
  // Export configuration (for debugging)
  export(): string {
    // Remove sensitive data
    const exportConfig = { ...this.config };
    delete exportConfig.ENCRYPTION_KEY;
    delete exportConfig.SENTRY_DSN;
    
    return JSON.stringify(exportConfig, null, 2);
  }
  
  // Get all configuration
  getAll(): EnvConfig {
    return { ...this.config };
  }
}

// Create singleton instance
export const config = new Config();

// Export commonly used values
export const APP_NAME = config.get('APP_NAME');
export const APP_VERSION = config.get('APP_VERSION');
export const API_BASE_URL = config.get('API_BASE_URL');
export const IS_PRODUCTION = config.isProduction();
export const IS_DEVELOPMENT = config.isDevelopment();
export const DEBUG = config.get('DEBUG');

// Export feature flags
export const FEATURES = config.get('FEATURES');

export default config;