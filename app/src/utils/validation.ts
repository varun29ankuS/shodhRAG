/**
 * Production-grade input validation and sanitization
 */

import { logger } from './logger';

// ============= Sanitization Functions =============

/**
 * Sanitize HTML to prevent XSS attacks
 */
export const sanitizeHtml = (input: string): string => {
  const div = document.createElement('div');
  div.textContent = input;
  return div.innerHTML;
};

/**
 * Sanitize SQL-like input to prevent injection
 */
export const sanitizeSqlInput = (input: string): string => {
  return input
    .replace(/['";\\]/g, '')
    .replace(/--/g, '')
    .replace(/\/\*/g, '')
    .replace(/\*\//g, '')
    .replace(/xp_/gi, '')
    .replace(/sp_/gi, '')
    .trim();
};

/**
 * Sanitize file paths to prevent directory traversal
 */
export const sanitizeFilePath = (path: string): string => {
  return path
    .replace(/\.\./g, '')
    .replace(/~\//, '')
    .replace(/^\/+/, '')
    .replace(/\\/g, '/')
    .replace(/\/+/g, '/')
    .trim();
};

/**
 * Sanitize URLs
 */
export const sanitizeUrl = (url: string): string | null => {
  try {
    const parsed = new URL(url);
    // Only allow http and https protocols
    if (!['http:', 'https:'].includes(parsed.protocol)) {
      logger.warn('Invalid URL protocol', { url, protocol: parsed.protocol });
      return null;
    }
    return parsed.toString();
  } catch (error) {
    logger.error('Invalid URL', { url, error });
    return null;
  }
};

/**
 * Escape special regex characters
 */
export const escapeRegex = (str: string): string => {
  return str.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
};

// ============= Validation Functions =============

export interface ValidationResult {
  isValid: boolean;
  errors: string[];
  sanitized?: unknown;
}

/**
 * Validate email address
 */
export const validateEmail = (email: string): ValidationResult => {
  const errors: string[] = [];
  const trimmed = email.trim().toLowerCase();
  
  if (!trimmed) {
    errors.push('Email is required');
  } else if (trimmed.length > 254) {
    errors.push('Email is too long');
  } else if (!/^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(trimmed)) {
    errors.push('Invalid email format');
  }
  
  return {
    isValid: errors.length === 0,
    errors,
    sanitized: trimmed,
  };
};

/**
 * Validate password strength
 */
export const validatePassword = (password: string): ValidationResult => {
  const errors: string[] = [];
  
  if (password.length < 8) {
    errors.push('Password must be at least 8 characters');
  }
  if (!/[A-Z]/.test(password)) {
    errors.push('Password must contain at least one uppercase letter');
  }
  if (!/[a-z]/.test(password)) {
    errors.push('Password must contain at least one lowercase letter');
  }
  if (!/[0-9]/.test(password)) {
    errors.push('Password must contain at least one number');
  }
  if (!/[!@#$%^&*(),.?":{}|<>]/.test(password)) {
    errors.push('Password must contain at least one special character');
  }
  
  return {
    isValid: errors.length === 0,
    errors,
  };
};

/**
 * Validate username
 */
export const validateUsername = (username: string): ValidationResult => {
  const errors: string[] = [];
  const sanitized = username.trim().toLowerCase();
  
  if (!sanitized) {
    errors.push('Username is required');
  } else if (sanitized.length < 3) {
    errors.push('Username must be at least 3 characters');
  } else if (sanitized.length > 30) {
    errors.push('Username must be less than 30 characters');
  } else if (!/^[a-z0-9_-]+$/.test(sanitized)) {
    errors.push('Username can only contain letters, numbers, underscores, and hyphens');
  }
  
  return {
    isValid: errors.length === 0,
    errors,
    sanitized,
  };
};

/**
 * Validate file upload
 */
export interface FileValidationOptions {
  maxSize?: number; // in bytes
  allowedTypes?: string[];
  allowedExtensions?: string[];
}

export const validateFile = (
  file: File,
  options: FileValidationOptions = {}
): ValidationResult => {
  const errors: string[] = [];
  const {
    maxSize = 10 * 1024 * 1024, // 10MB default
    allowedTypes = [],
    allowedExtensions = [],
  } = options;
  
  if (file.size > maxSize) {
    errors.push(`File size exceeds ${formatFileSize(maxSize)}`);
  }
  
  if (allowedTypes.length > 0 && !allowedTypes.includes(file.type)) {
    errors.push(`File type ${file.type} is not allowed`);
  }
  
  const extension = file.name.split('.').pop()?.toLowerCase();
  if (allowedExtensions.length > 0 && extension && !allowedExtensions.includes(extension)) {
    errors.push(`File extension .${extension} is not allowed`);
  }
  
  // Check for potentially dangerous file names
  if (/[<>:"|?*]/.test(file.name)) {
    errors.push('File name contains invalid characters');
  }
  
  return {
    isValid: errors.length === 0,
    errors,
  };
};

/**
 * Validate search query
 */
export const validateSearchQuery = (query: string): ValidationResult => {
  const errors: string[] = [];
  const sanitized = query.trim();
  
  if (!sanitized) {
    errors.push('Search query is required');
  } else if (sanitized.length < 2) {
    errors.push('Search query must be at least 2 characters');
  } else if (sanitized.length > 200) {
    errors.push('Search query is too long');
  }
  
  // Remove potentially dangerous characters
  const cleaned = sanitized
    .replace(/[<>]/g, '')
    .replace(/javascript:/gi, '')
    .replace(/on\w+=/gi, '');
  
  return {
    isValid: errors.length === 0,
    errors,
    sanitized: cleaned,
  };
};

/**
 * Validate space name
 */
export const validateSpaceName = (name: string): ValidationResult => {
  const errors: string[] = [];
  const sanitized = name.trim();
  
  if (!sanitized) {
    errors.push('Space name is required');
  } else if (sanitized.length < 2) {
    errors.push('Space name must be at least 2 characters');
  } else if (sanitized.length > 50) {
    errors.push('Space name must be less than 50 characters');
  } else if (/^[._]/.test(sanitized)) {
    errors.push('Space name cannot start with . or _');
  }
  
  return {
    isValid: errors.length === 0,
    errors,
    sanitized,
  };
};

// ============= Form Validation =============

export interface FormField {
  name: string;
  value: unknown;
  rules?: ValidationRules;
}

export interface ValidationRules {
  required?: boolean | string;
  minLength?: number | [number, string];
  maxLength?: number | [number, string];
  pattern?: RegExp | [RegExp, string];
  custom?: (value: unknown) => boolean | string;
  email?: boolean;
  url?: boolean;
  number?: boolean;
  integer?: boolean;
  min?: number;
  max?: number;
}

/**
 * Validate a single form field
 */
export const validateField = (field: FormField): string[] => {
  const { value, rules } = field;
  const errors: string[] = [];
  
  if (!rules) return errors;
  
  // Required validation
  if (rules.required) {
    const isEmpty = value === null || value === undefined || value === '' ||
                    (Array.isArray(value) && value.length === 0);
    if (isEmpty) {
      errors.push(typeof rules.required === 'string' ? rules.required : 'This field is required');
    }
  }
  
  // Skip other validations if value is empty and not required
  if (!value && !rules.required) return errors;
  
  const stringValue = String(value);
  
  // Length validations
  if (rules.minLength) {
    const [min, message] = Array.isArray(rules.minLength) ? rules.minLength : [rules.minLength, `Minimum length is ${rules.minLength}`];
    if (stringValue.length < min) {
      errors.push(message);
    }
  }
  
  if (rules.maxLength) {
    const [max, message] = Array.isArray(rules.maxLength) ? rules.maxLength : [rules.maxLength, `Maximum length is ${rules.maxLength}`];
    if (stringValue.length > max) {
      errors.push(message);
    }
  }
  
  // Pattern validation
  if (rules.pattern) {
    const [pattern, message] = Array.isArray(rules.pattern) ? rules.pattern : [rules.pattern, 'Invalid format'];
    if (!pattern.test(stringValue)) {
      errors.push(message);
    }
  }
  
  // Email validation
  if (rules.email && !/^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(stringValue)) {
    errors.push('Invalid email address');
  }
  
  // URL validation
  if (rules.url) {
    try {
      new URL(stringValue);
    } catch {
      errors.push('Invalid URL');
    }
  }
  
  // Number validations
  if (rules.number && isNaN(Number(value))) {
    errors.push('Must be a number');
  }
  
  if (rules.integer && !Number.isInteger(Number(value))) {
    errors.push('Must be an integer');
  }
  
  if (rules.min !== undefined && Number(value) < rules.min) {
    errors.push(`Minimum value is ${rules.min}`);
  }
  
  if (rules.max !== undefined && Number(value) > rules.max) {
    errors.push(`Maximum value is ${rules.max}`);
  }
  
  // Custom validation
  if (rules.custom) {
    const result = rules.custom(value);
    if (typeof result === 'string') {
      errors.push(result);
    } else if (!result) {
      errors.push('Validation failed');
    }
  }
  
  return errors;
};

/**
 * Validate entire form
 */
export const validateForm = (fields: FormField[]): Record<string, string[]> => {
  const errors: Record<string, string[]> = {};
  
  for (const field of fields) {
    const fieldErrors = validateField(field);
    if (fieldErrors.length > 0) {
      errors[field.name] = fieldErrors;
    }
  }
  
  return errors;
};

// ============= Rate Limiting =============

interface RateLimitEntry {
  count: number;
  resetTime: number;
}

class RateLimiter {
  private limits: Map<string, RateLimitEntry> = new Map();
  
  check(key: string, maxRequests: number, windowMs: number): boolean {
    const now = Date.now();
    const entry = this.limits.get(key);
    
    if (!entry || now > entry.resetTime) {
      this.limits.set(key, {
        count: 1,
        resetTime: now + windowMs,
      });
      return true;
    }
    
    if (entry.count >= maxRequests) {
      logger.warn('Rate limit exceeded', { key, count: entry.count, maxRequests });
      return false;
    }
    
    entry.count++;
    return true;
  }
  
  reset(key: string): void {
    this.limits.delete(key);
  }
  
  resetAll(): void {
    this.limits.clear();
  }
}

export const rateLimiter = new RateLimiter();

// ============= Utility Functions =============

/**
 * Format file size for display
 */
export const formatFileSize = (bytes: number): string => {
  if (bytes === 0) return '0 B';
  const k = 1024;
  const sizes = ['B', 'KB', 'MB', 'GB', 'TB'];
  const i = Math.floor(Math.log(bytes) / Math.log(k));
  return `${parseFloat((bytes / Math.pow(k, i)).toFixed(2))} ${sizes[i]}`;
};

/**
 * Debounce function for input validation
 */
export const debounce = <T extends (...args: any[]) => any>(
  func: T,
  wait: number
): ((...args: Parameters<T>) => void) => {
  let timeout: NodeJS.Timeout;
  
  return (...args: Parameters<T>) => {
    clearTimeout(timeout);
    timeout = setTimeout(() => func(...args), wait);
  };
};

/**
 * Throttle function for rate limiting
 */
export const throttle = <T extends (...args: any[]) => any>(
  func: T,
  limit: number
): ((...args: Parameters<T>) => void) => {
  let inThrottle = false;
  
  return (...args: Parameters<T>) => {
    if (!inThrottle) {
      func(...args);
      inThrottle = true;
      setTimeout(() => {
        inThrottle = false;
      }, limit);
    }
  };
};

export default {
  sanitizeHtml,
  sanitizeSqlInput,
  sanitizeFilePath,
  sanitizeUrl,
  escapeRegex,
  validateEmail,
  validatePassword,
  validateUsername,
  validateFile,
  validateSearchQuery,
  validateSpaceName,
  validateField,
  validateForm,
  rateLimiter,
  formatFileSize,
  debounce,
  throttle,
};