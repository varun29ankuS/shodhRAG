import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { save } from '@tauri-apps/plugin-dialog';
import { writeFile } from '@tauri-apps/plugin-fs';

export interface GenerateDocumentRequest {
  format: string;
  templateId?: string;
  data: any;
  query?: string;
  useRag: boolean;
}

export interface GenerateDocumentResponse {
  id: string;
  title: string;
  format: string;
  size: number;
  pages?: number;
  preview?: string;
  contentBase64?: string;
  metadata: {
    createdAt: string;
    sources: string[];
    template?: string;
    generationTimeMs: number;
  };
}

export interface GenerationStatusUpdate {
  stepId: string;
  stepName: string;
  description: string;
  status: 'pending' | 'running' | 'complete' | 'error';
  progress?: number;
  result?: any;
}

export interface TemplateInfo {
  id: string;
  name: string;
  description: string;
  format: string;
}

class DocumentGenerationService {
  private statusListeners: Map<string, (status: GenerationStatusUpdate) => void> = new Map();
  private unlistenStatus: (() => void) | null = null;

  constructor() {
    this.setupEventListeners();
  }

  private async setupEventListeners() {
    // Listen for generation status updates
    this.unlistenStatus = await listen<GenerationStatusUpdate>('generation_status', (event) => {
      // Notify all registered listeners
      this.statusListeners.forEach(listener => listener(event.payload));
    });
  }

  /**
   * Generate a document with the specified format and data
   */
  async generateDocument(
    format: string,
    data: any,
    options?: {
      templateId?: string;
      query?: string;
      useRag?: boolean;
      onStatus?: (status: GenerationStatusUpdate) => void;
    }
  ): Promise<GenerateDocumentResponse> {
    // Register status listener if provided
    const listenerId = options?.onStatus ? Date.now().toString() : null;
    if (listenerId && options?.onStatus) {
      this.statusListeners.set(listenerId, options.onStatus);
    }

    try {
      const request: GenerateDocumentRequest = {
        format,
        data,
        templateId: options?.templateId,
        query: options?.query,
        useRag: options?.useRag ?? false,
      };

      const response = await invoke<GenerateDocumentResponse>('generate_document', {
        request
      });

      return response;
    } finally {
      // Clean up listener
      if (listenerId) {
        this.statusListeners.delete(listenerId);
      }
    }
  }

  /**
   * Generate a document from RAG search results
   */
  async generateFromRAG(
    query: string,
    format: string,
    templateId?: string,
    onStatus?: (status: GenerationStatusUpdate) => void
  ): Promise<GenerateDocumentResponse> {
    // Register status listener
    const listenerId = onStatus ? Date.now().toString() : null;
    if (listenerId && onStatus) {
      this.statusListeners.set(listenerId, onStatus);
    }

    try {
      const response = await invoke<GenerateDocumentResponse>('generate_from_rag', {
        query,
        format,
        template: templateId
      });

      return response;
    } finally {
      // Clean up listener
      if (listenerId) {
        this.statusListeners.delete(listenerId);
      }
    }
  }

  /**
   * Get available document formats
   */
  async getAvailableFormats(): Promise<string[]> {
    return await invoke<string[]>('get_available_formats');
  }

  /**
   * Get available templates
   */
  async getAvailableTemplates(): Promise<TemplateInfo[]> {
    return await invoke<TemplateInfo[]>('get_available_templates');
  }

  /**
   * Generate a preview for a document
   */
  async generatePreview(documentId: string, format: string): Promise<string> {
    return await invoke<string>('generate_document_preview', {
      documentId,
      format
    });
  }

  /**
   * Get source documents used in generation
   */
  async getSourceDocuments(documentIds: string[]): Promise<any[]> {
    return await invoke<any[]>('get_source_documents', {
      documentIds
    });
  }

  /**
   * Download a generated document
   */
  async downloadDocument(document: GenerateDocumentResponse): Promise<void> {
    // Open save dialog
    const filePath = await save({
      defaultPath: document.title,
      filters: [{
        name: document.format.toUpperCase(),
        extensions: [document.format.toLowerCase()]
      }]
    });

    if (filePath && document.contentBase64) {
      // Decode base64 and save
      const bytes = this.base64ToUint8Array(document.contentBase64);
      await writeFile(filePath, bytes);
    }
  }

  /**
   * Convert base64 to Uint8Array
   */
  private base64ToUint8Array(base64: string): Uint8Array {
    const binaryString = atob(base64);
    const bytes = new Uint8Array(binaryString.length);
    for (let i = 0; i < binaryString.length; i++) {
      bytes[i] = binaryString.charCodeAt(i);
    }
    return bytes;
  }

  /**
   * Create a blob from document for preview
   */
  createDocumentBlob(document: GenerateDocumentResponse): Blob {
    if (document.contentBase64) {
      const bytes = this.base64ToUint8Array(document.contentBase64);
      const mimeType = this.getMimeType(document.format);
      return new Blob([bytes], { type: mimeType });
    }
    return new Blob();
  }

  /**
   * Get MIME type for format
   */
  private getMimeType(format: string): string {
    const mimeTypes: Record<string, string> = {
      pdf: 'application/pdf',
      docx: 'application/vnd.openxmlformats-officedocument.wordprocessingml.document',
      doc: 'application/msword',
      xlsx: 'application/vnd.openxmlformats-officedocument.spreadsheetml.sheet',
      xls: 'application/vnd.ms-excel',
      pptx: 'application/vnd.openxmlformats-officedocument.presentationml.presentation',
      ppt: 'application/vnd.ms-powerpoint',
      csv: 'text/csv',
      json: 'application/json',
      xml: 'application/xml',
      html: 'text/html',
      md: 'text/markdown',
      txt: 'text/plain',
    };
    return mimeTypes[format.toLowerCase()] || 'application/octet-stream';
  }

  /**
   * Quick generation methods for common formats
   */
  async generatePDF(data: any, templateId?: string): Promise<GenerateDocumentResponse> {
    return this.generateDocument('pdf', data, { templateId });
  }

  async generateWord(data: any, templateId?: string): Promise<GenerateDocumentResponse> {
    return this.generateDocument('docx', data, { templateId });
  }

  async generateExcel(data: any, templateId?: string): Promise<GenerateDocumentResponse> {
    return this.generateDocument('xlsx', data, { templateId });
  }

  async generateMarkdown(data: any, templateId?: string): Promise<GenerateDocumentResponse> {
    return this.generateDocument('md', data, { templateId });
  }

  /**
   * Generate documents from search results with specific templates
   */
  async generateComplianceReport(searchResults: any[]): Promise<GenerateDocumentResponse> {
    return this.generateDocument('pdf', {
      searchResults,
      reportType: 'compliance'
    }, {
      templateId: 'compliance_report',
      useRag: true
    });
  }

  async generateExecutiveSummary(searchResults: any[]): Promise<GenerateDocumentResponse> {
    return this.generateDocument('docx', {
      searchResults,
      reportType: 'executive'
    }, {
      templateId: 'executive_summary',
      useRag: true
    });
  }

  async generateDataExport(searchResults: any[]): Promise<GenerateDocumentResponse> {
    return this.generateDocument('xlsx', {
      searchResults,
      includeMetadata: true
    }, {
      useRag: false
    });
  }

  /**
   * Cleanup
   */
  destroy() {
    if (this.unlistenStatus) {
      this.unlistenStatus();
    }
    this.statusListeners.clear();
  }
}

// Export singleton instance
export const documentGenerationService = new DocumentGenerationService();

export default documentGenerationService;