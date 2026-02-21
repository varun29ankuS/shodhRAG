import { useState, useCallback } from 'react';
import { documentGenerationService, GenerateDocumentResponse, GenerationStatusUpdate } from '../services/DocumentGenerationService';
import { GenerationStep } from '../components/GenerationStatus';

interface UseDocumentGenerationOptions {
  onComplete?: (document: GenerateDocumentResponse) => void;
  onError?: (error: string) => void;
  onStatusUpdate?: (status: GenerationStatusUpdate) => void;
}

export const useDocumentGeneration = (options?: UseDocumentGenerationOptions) => {
  const [isGenerating, setIsGenerating] = useState(false);
  const [generationSteps, setGenerationSteps] = useState<GenerationStep[]>([]);
  const [generatedDocument, setGeneratedDocument] = useState<GenerateDocumentResponse | null>(null);
  const [error, setError] = useState<string | null>(null);

  const updateStep = useCallback((statusUpdate: GenerationStatusUpdate) => {
    setGenerationSteps(prev => {
      const existingIndex = prev.findIndex(s => s.id === statusUpdate.stepId);
      
      const step: GenerationStep = {
        id: statusUpdate.stepId,
        name: statusUpdate.stepName,
        description: statusUpdate.description,
        status: statusUpdate.status,
        progress: statusUpdate.progress,
        result: statusUpdate.result,
      };

      if (existingIndex >= 0) {
        const updated = [...prev];
        updated[existingIndex] = step;
        return updated;
      } else {
        return [...prev, step];
      }
    });

    options?.onStatusUpdate?.(statusUpdate);
  }, [options]);

  const generateDocument = useCallback(async (
    format: string,
    data: any,
    config?: {
      templateId?: string;
      query?: string;
      useRag?: boolean;
    }
  ) => {
    setIsGenerating(true);
    setError(null);
    setGenerationSteps([]);
    setGeneratedDocument(null);

    try {
      const response = await documentGenerationService.generateDocument(
        format,
        data,
        {
          ...config,
          onStatus: updateStep
        }
      );

      setGeneratedDocument(response);
      options?.onComplete?.(response);
      return response;
    } catch (err) {
      const errorMessage = err instanceof Error ? err.message : 'Document generation failed';
      setError(errorMessage);
      options?.onError?.(errorMessage);
      throw err;
    } finally {
      setIsGenerating(false);
    }
  }, [updateStep, options]);

  const generateFromRAG = useCallback(async (
    query: string,
    format: string,
    templateId?: string
  ) => {
    setIsGenerating(true);
    setError(null);
    setGenerationSteps([]);
    setGeneratedDocument(null);

    try {
      const response = await documentGenerationService.generateFromRAG(
        query,
        format,
        templateId,
        updateStep
      );

      setGeneratedDocument(response);
      options?.onComplete?.(response);
      return response;
    } catch (err) {
      const errorMessage = err instanceof Error ? err.message : 'Document generation failed';
      setError(errorMessage);
      options?.onError?.(errorMessage);
      throw err;
    } finally {
      setIsGenerating(false);
    }
  }, [updateStep, options]);

  const downloadDocument = useCallback(async () => {
    if (!generatedDocument) return;

    try {
      await documentGenerationService.downloadDocument(generatedDocument);
    } catch (err) {
      console.error('Failed to download document:', err);
    }
  }, [generatedDocument]);

  const createPreviewUrl = useCallback(() => {
    if (!generatedDocument) return null;

    const blob = documentGenerationService.createDocumentBlob(generatedDocument);
    return URL.createObjectURL(blob);
  }, [generatedDocument]);

  return {
    // State
    isGenerating,
    generationSteps,
    generatedDocument,
    error,

    // Actions
    generateDocument,
    generateFromRAG,
    downloadDocument,
    createPreviewUrl,

    // Quick generators
    generatePDF: (data: any, templateId?: string) => generateDocument('pdf', data, { templateId }),
    generateWord: (data: any, templateId?: string) => generateDocument('docx', data, { templateId }),
    generateExcel: (data: any, templateId?: string) => generateDocument('xlsx', data, { templateId }),
    generateMarkdown: (data: any, templateId?: string) => generateDocument('md', data, { templateId }),
  };
};

export default useDocumentGeneration;