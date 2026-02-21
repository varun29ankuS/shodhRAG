import { toast } from 'sonner';

export interface ValidationResult {
  valid: boolean;
  error?: string;
}

export function isValidPath(path: string): ValidationResult {
  if (!path || typeof path !== 'string') {
    return { valid: false, error: 'Path is required' };
  }

  const trimmed = path.trim();
  if (trimmed.length === 0) {
    return { valid: false, error: 'Path cannot be empty' };
  }

  const invalidChars = /[<>"|?*]/;
  if (invalidChars.test(trimmed)) {
    return { valid: false, error: 'Path contains invalid characters' };
  }

  if (trimmed.length > 260) {
    return { valid: false, error: 'Path is too long (max 260 characters)' };
  }

  return { valid: true };
}

export function isValidFileName(fileName: string): ValidationResult {
  if (!fileName || typeof fileName !== 'string') {
    return { valid: false, error: 'File name is required' };
  }

  const trimmed = fileName.trim();
  if (trimmed.length === 0) {
    return { valid: false, error: 'File name cannot be empty' };
  }

  const invalidChars = /[<>:"/\\|?*\x00-\x1F]/;
  if (invalidChars.test(trimmed)) {
    return { valid: false, error: 'File name contains invalid characters' };
  }

  const reservedNames = /^(CON|PRN|AUX|NUL|COM[1-9]|LPT[1-9])$/i;
  if (reservedNames.test(trimmed)) {
    return { valid: false, error: 'File name is reserved by the system' };
  }

  if (trimmed.length > 255) {
    return { valid: false, error: 'File name is too long (max 255 characters)' };
  }

  return { valid: true };
}

export function isValidFileSize(sizeBytes: number, maxMB: number = 100): ValidationResult {
  if (typeof sizeBytes !== 'number' || sizeBytes < 0) {
    return { valid: false, error: 'Invalid file size' };
  }

  const maxBytes = maxMB * 1024 * 1024;
  if (sizeBytes > maxBytes) {
    return { valid: false, error: `File size exceeds ${maxMB}MB limit` };
  }

  return { valid: true };
}

export function isValidFileType(fileName: string, allowedExtensions: string[]): ValidationResult {
  if (!fileName || typeof fileName !== 'string') {
    return { valid: false, error: 'File name is required' };
  }

  const extension = fileName.split('.').pop()?.toLowerCase();
  if (!extension) {
    return { valid: false, error: 'File has no extension' };
  }

  if (!allowedExtensions.includes(extension)) {
    return {
      valid: false,
      error: `File type .${extension} is not allowed. Allowed types: ${allowedExtensions.join(', ')}`,
    };
  }

  return { valid: true };
}

export function sanitizeInput(input: string): string {
  if (typeof input !== 'string') return '';

  return input
    .trim()
    .replace(/[<>]/g, '')
    .replace(/javascript:/gi, '')
    .replace(/on\w+=/gi, '')
    .slice(0, 10000);
}

export function sanitizePath(path: string): string {
  if (typeof path !== 'string') return '';

  return path
    .trim()
    .replace(/\\/g, '/')
    .replace(/\/+/g, '/')
    .replace(/^\/+/, '')
    .replace(/\/+$/, '');
}

export function isValidUrl(url: string): ValidationResult {
  if (!url || typeof url !== 'string') {
    return { valid: false, error: 'URL is required' };
  }

  try {
    const parsed = new URL(url);
    const allowedProtocols = ['http:', 'https:'];

    if (!allowedProtocols.includes(parsed.protocol)) {
      return { valid: false, error: 'URL must use HTTP or HTTPS protocol' };
    }

    return { valid: true };
  } catch {
    return { valid: false, error: 'Invalid URL format' };
  }
}

export function isValidEmail(email: string): ValidationResult {
  if (!email || typeof email !== 'string') {
    return { valid: false, error: 'Email is required' };
  }

  const emailRegex = /^[^\s@]+@[^\s@]+\.[^\s@]+$/;
  if (!emailRegex.test(email)) {
    return { valid: false, error: 'Invalid email format' };
  }

  if (email.length > 254) {
    return { valid: false, error: 'Email is too long' };
  }

  return { valid: true };
}

export function validateAndShow(result: ValidationResult, showToast: boolean = true): boolean {
  if (!result.valid && showToast && result.error) {
    toast.error(result.error);
  }
  return result.valid;
}

export class Validator {
  private errors: string[] = [];
  private showToasts: boolean;

  constructor(showToasts: boolean = true) {
    this.showToasts = showToasts;
  }

  path(path: string, fieldName: string = 'Path'): this {
    const result = isValidPath(path);
    if (!result.valid) {
      this.errors.push(`${fieldName}: ${result.error}`);
    }
    return this;
  }

  fileName(fileName: string, fieldName: string = 'File name'): this {
    const result = isValidFileName(fileName);
    if (!result.valid) {
      this.errors.push(`${fieldName}: ${result.error}`);
    }
    return this;
  }

  fileSize(sizeBytes: number, maxMB: number = 100, fieldName: string = 'File'): this {
    const result = isValidFileSize(sizeBytes, maxMB);
    if (!result.valid) {
      this.errors.push(`${fieldName}: ${result.error}`);
    }
    return this;
  }

  fileType(
    fileName: string,
    allowedExtensions: string[],
    fieldName: string = 'File'
  ): this {
    const result = isValidFileType(fileName, allowedExtensions);
    if (!result.valid) {
      this.errors.push(`${fieldName}: ${result.error}`);
    }
    return this;
  }

  url(url: string, fieldName: string = 'URL'): this {
    const result = isValidUrl(url);
    if (!result.valid) {
      this.errors.push(`${fieldName}: ${result.error}`);
    }
    return this;
  }

  email(email: string, fieldName: string = 'Email'): this {
    const result = isValidEmail(email);
    if (!result.valid) {
      this.errors.push(`${fieldName}: ${result.error}`);
    }
    return this;
  }

  custom(predicate: boolean, errorMessage: string): this {
    if (!predicate) {
      this.errors.push(errorMessage);
    }
    return this;
  }

  required(value: any, fieldName: string): this {
    if (value === null || value === undefined || value === '') {
      this.errors.push(`${fieldName} is required`);
    }
    return this;
  }

  minLength(value: string, min: number, fieldName: string): this {
    if (typeof value === 'string' && value.length < min) {
      this.errors.push(`${fieldName} must be at least ${min} characters`);
    }
    return this;
  }

  maxLength(value: string, max: number, fieldName: string): this {
    if (typeof value === 'string' && value.length > max) {
      this.errors.push(`${fieldName} must be at most ${max} characters`);
    }
    return this;
  }

  isValid(): boolean {
    if (this.errors.length > 0 && this.showToasts) {
      this.errors.forEach((error) => toast.error(error));
    }
    return this.errors.length === 0;
  }

  getErrors(): string[] {
    return [...this.errors];
  }

  getFirstError(): string | undefined {
    return this.errors[0];
  }
}

export function validate(showToasts: boolean = true): Validator {
  return new Validator(showToasts);
}
