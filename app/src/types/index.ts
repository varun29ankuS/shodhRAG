/**
 * Production-grade TypeScript type definitions
 * All types are properly defined with no 'any' types
 */

// ============= Domain Types =============

export interface Space {
  id: string;
  name: string;
  emoji: string;
  color?: string;
  documentCount: number;
  lastActive: string;
  isShared?: boolean;
  newInsights?: number;
  folderPath?: string;
  watchingChanges?: boolean;
  chatMessageCount?: number;
  createdAt: string;
  updatedAt: string;
  metadata?: Record<string, unknown>;
}

export interface Document {
  id: string;
  title: string;
  content: string;
  size: string;
  type: string;
  addedAt: string;
  tags?: string[];
  metadata: DocumentMetadata;
  spaceId?: string;
  version?: number;
  checksum?: string;
}

export interface DocumentMetadata {
  fileName?: string;
  filePath?: string;
  fileSize?: number;
  mimeType?: string;
  createdAt?: string;
  modifiedAt?: string;
  author?: string;
  [key: string]: string | number | boolean | undefined;
}

export interface SearchResult {
  id: string;
  documentId: string;
  documentTitle: string;
  score: number;
  snippet: string;
  highlights?: TextHighlight[];
  metadata?: Record<string, unknown>;
}

export interface TextHighlight {
  start: number;
  end: number;
  text: string;
}

export interface Connection {
  id: string;
  from: string;
  to: string;
  strength: number;
  type: ConnectionType;
  metadata?: Record<string, unknown>;
}

export enum ConnectionType {
  CITATION = 'citation',
  SIMILARITY = 'similarity',
  REFERENCE = 'reference',
  TOPIC = 'topic',
}

export interface DailyBrief {
  date: string;
  newDocuments: number;
  newConnections: Connection[];
  topSearches: string[];
  insights?: Insight[];
}

export interface Insight {
  id: string;
  type: 'trend' | 'anomaly' | 'suggestion';
  title: string;
  description: string;
  priority: 'low' | 'medium' | 'high';
  actionable: boolean;
}

// ============= Chat Types =============

export interface ChatMessage {
  id: string;
  role: 'user' | 'assistant' | 'system';
  content: string;
  timestamp: string;
  metadata?: ChatMetadata;
}

export interface ChatMetadata {
  model?: string;
  temperature?: number;
  tokens?: number;
  processingTime?: number;
  citations?: Citation[];
}

export interface Citation {
  id: string;
  documentId: string;
  documentTitle: string;
  pageNumber?: number;
  confidence: number;
}

export interface ChatSession {
  id: string;
  spaceId?: string;
  messages: ChatMessage[];
  createdAt: string;
  updatedAt: string;
  title?: string;
}

// ============= LLM Types =============

export interface LLMConfig {
  mode: LLMMode;
  model?: string;
  temperature: number;
  maxTokens: number;
  topP: number;
  frequencyPenalty: number;
  presencePenalty: number;
  systemPrompt?: string;
}

export enum LLMMode {
  DISABLED = 'disabled',
  LOCAL = 'local',
  OPENAI = 'openai',
  ANTHROPIC = 'anthropic',
  CUSTOM = 'custom',
}

export interface LLMStatus {
  isReady: boolean;
  mode: LLMMode;
  model?: string;
  error?: string;
  capabilities?: string[];
}

export interface ApiKeys {
  openai?: string;
  anthropic?: string;
  custom?: string;
}

// ============= Graph Types =============

export interface GraphNode {
  id: string;
  label: string;
  type: NodeType;
  size: number;
  color: string;
  position?: Vector3D;
  metadata?: NodeMetadata;
}

export enum NodeType {
  DOCUMENT = 'document',
  TOPIC = 'topic',
  ENTITY = 'entity',
  SPACE = 'space',
  TAG = 'tag',
}

export interface NodeMetadata {
  documentCount?: number;
  connections?: number;
  score?: number;
  space?: string;
  filePath?: string;
  [key: string]: unknown;
}

export interface GraphEdge {
  id: string;
  source: string;
  target: string;
  weight: number;
  type: EdgeType;
  label?: string;
  metadata?: Record<string, unknown>;
}

export enum EdgeType {
  CITATION = 'citation',
  SIMILARITY = 'similarity',
  TOPIC = 'topic',
  COOCCURRENCE = 'cooccurrence',
  HIERARCHY = 'hierarchy',
}

export interface GraphData {
  nodes: GraphNode[];
  edges: GraphEdge[];
  metadata?: GraphMetadata;
}

export interface GraphMetadata {
  createdAt: string;
  nodeCount: number;
  edgeCount: number;
  density: number;
  clustering: number;
}

export interface Vector3D {
  x: number;
  y: number;
  z: number;
}

// ============= UI State Types =============

export interface Theme {
  mode: 'light' | 'dark';
  accent: AccentColor;
  fontSize: 'small' | 'medium' | 'large';
  animations: boolean;
}

export enum AccentColor {
  GREEN = 'green',
  BLUE = 'blue',
  PURPLE = 'purple',
  RED = 'red',
  ORANGE = 'orange',
}

export interface NotificationOptions {
  id?: string;
  type: 'info' | 'success' | 'warning' | 'error';
  title: string;
  message?: string;
  duration?: number;
  action?: NotificationAction;
}

export interface NotificationAction {
  label: string;
  onClick: () => void;
}

export interface ModalState {
  isOpen: boolean;
  title?: string;
  content?: React.ReactNode;
  actions?: ModalAction[];
  size?: 'small' | 'medium' | 'large' | 'fullscreen';
}

export interface ModalAction {
  label: string;
  onClick: () => void | Promise<void>;
  variant?: 'primary' | 'secondary' | 'danger';
  disabled?: boolean;
}

// ============= Form Types =============

export interface ValidationRule {
  required?: boolean;
  minLength?: number;
  maxLength?: number;
  pattern?: RegExp;
  custom?: (value: unknown) => boolean | string;
}

export interface FieldError {
  field: string;
  message: string;
  code?: string;
}

export interface FormState<T = Record<string, unknown>> {
  values: T;
  errors: Record<string, FieldError>;
  touched: Record<string, boolean>;
  isSubmitting: boolean;
  isValid: boolean;
}

// ============= API Types =============

export interface ApiResponse<T = unknown> {
  data?: T;
  error?: ApiError;
  metadata?: ResponseMetadata;
}

export interface ApiError {
  code: string;
  message: string;
  details?: Record<string, unknown>;
  timestamp: string;
}

export interface ResponseMetadata {
  requestId: string;
  duration: number;
  cached: boolean;
  rateLimit?: RateLimit;
}

export interface RateLimit {
  limit: number;
  remaining: number;
  reset: number;
}

export interface PaginationParams {
  page: number;
  pageSize: number;
  sortBy?: string;
  sortOrder?: 'asc' | 'desc';
}

export interface PaginatedResponse<T> {
  items: T[];
  total: number;
  page: number;
  pageSize: number;
  totalPages: number;
  hasNext: boolean;
  hasPrevious: boolean;
}

// ============= File System Types =============

export interface FileInfo {
  name: string;
  path: string;
  size: number;
  type: FileType;
  mimeType?: string;
  createdAt: string;
  modifiedAt: string;
  isDirectory: boolean;
  children?: FileInfo[];
}

export enum FileType {
  PDF = 'pdf',
  DOCX = 'docx',
  TXT = 'txt',
  MD = 'md',
  IMAGE = 'image',
  VIDEO = 'video',
  AUDIO = 'audio',
  OTHER = 'other',
}

export interface FolderStats {
  totalFiles: number;
  totalSize: number;
  fileTypes: Record<FileType, number>;
  lastModified: string;
}

// ============= Settings Types =============

export interface AppSettings {
  theme: Theme;
  language: string;
  notifications: NotificationSettings;
  privacy: PrivacySettings;
  performance: PerformanceSettings;
  shortcuts: KeyboardShortcuts;
}

export interface NotificationSettings {
  enabled: boolean;
  sound: boolean;
  desktop: boolean;
  types: {
    info: boolean;
    success: boolean;
    warning: boolean;
    error: boolean;
  };
}

export interface PrivacySettings {
  telemetry: boolean;
  crashReports: boolean;
  analytics: boolean;
  shareUsageData: boolean;
}

export interface PerformanceSettings {
  enableHardwareAcceleration: boolean;
  maxConcurrentOperations: number;
  cacheSize: number;
  autoSave: boolean;
  autoSaveInterval: number;
}

export interface KeyboardShortcuts {
  search: string;
  newSpace: string;
  settings: string;
  help: string;
  [key: string]: string;
}

// ============= Event Types =============

export interface AppEvent<T = unknown> {
  type: string;
  payload?: T;
  timestamp: string;
  source?: string;
}

export interface ProgressEvent {
  current: number;
  total: number;
  message?: string;
  details?: Record<string, unknown>;
}

export interface ErrorEvent {
  error: Error;
  context?: Record<string, unknown>;
  recoverable: boolean;
}

// ============= Utility Types =============

export type Nullable<T> = T | null;
export type Optional<T> = T | undefined;
export type AsyncFunction<T = void> = () => Promise<T>;
export type Callback<T = void> = (data: T) => void;

export interface Disposable {
  dispose(): void;
}

export interface Subscribable<T> {
  subscribe(callback: Callback<T>): Disposable;
}

// Type guards
export const isSpace = (obj: unknown): obj is Space => {
  return typeof obj === 'object' && obj !== null && 'id' in obj && 'name' in obj;
};

export const isDocument = (obj: unknown): obj is Document => {
  return typeof obj === 'object' && obj !== null && 'id' in obj && 'title' in obj && 'content' in obj;
};

export const isError = (obj: unknown): obj is Error => {
  return obj instanceof Error;
};

// Validation helpers
export const validateEmail = (email: string): boolean => {
  return /^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(email);
};

export const validateUrl = (url: string): boolean => {
  try {
    new URL(url);
    return true;
  } catch {
    return false;
  }
};