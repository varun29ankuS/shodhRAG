import React, { useState, useRef, useCallback } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Upload, Image as ImageIcon, X, Loader2, CheckCircle2, Clipboard } from 'lucide-react';
import { Button } from './ui/button';

interface ImageUploadProps {
  onImageProcessed?: (result: ImageProcessResult) => void;
  compact?: boolean;
}

interface ImageProcessResult {
  id: string;
  extracted_text: string;
  file_path: string;
  confidence: number;
  word_count: number;
}

export function ImageUpload({ onImageProcessed, compact = false }: ImageUploadProps) {
  const [isDragging, setIsDragging] = useState(false);
  const [isProcessing, setIsProcessing] = useState(false);
  const [uploadedImage, setUploadedImage] = useState<string | null>(null);
  const [processResult, setProcessResult] = useState<ImageProcessResult | null>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);

  // Handle file drop
  const handleDrop = useCallback(async (e: React.DragEvent) => {
    e.preventDefault();
    e.stopPropagation();
    setIsDragging(false);

    const files = Array.from(e.dataTransfer.files);
    const imageFile = files.find(f => f.type.startsWith('image/'));

    if (imageFile) {
      await processImageFile(imageFile);
    }
  }, []);

  // Process image file
  const processImageFile = async (file: File) => {
    setIsProcessing(true);
    setProcessResult(null);

    try {
      // Convert to base64
      const reader = new FileReader();
      reader.onload = async (e) => {
        const base64Data = e.target?.result as string;
        setUploadedImage(base64Data);

        try {
          // Call backend OCR
          const result = await invoke<ImageProcessResult>('process_image_from_base64', {
            imageData: base64Data
          });

          setProcessResult(result);
          onImageProcessed?.(result);
        } catch (error) {
          console.error('Failed to process image:', error);
          alert(`OCR failed: ${error}`);
        } finally {
          setIsProcessing(false);
        }
      };

      reader.readAsDataURL(file);
    } catch (error) {
      console.error('Failed to read image:', error);
      setIsProcessing(false);
    }
  };

  // Handle paste from clipboard button
  const handlePasteFromClipboard = useCallback(async () => {
    try {
      console.log('📋 Reading from clipboard using native Windows API...');
      setIsProcessing(true);

      // Read image from clipboard using our native Windows command
      const base64Data = await invoke<string | null>('read_clipboard_image');

      console.log('📦 Clipboard response:', {
        hasData: !!base64Data,
        dataLength: base64Data ? base64Data.length : 0
      });

      if (base64Data) {
        console.log('✅ Image found in clipboard');
        setUploadedImage(base64Data);

        try {
          const result = await invoke<ImageProcessResult>('process_image_from_base64', {
            imageData: base64Data
          });

          setProcessResult(result);
          onImageProcessed?.(result);
          console.log('✅ OCR processing completed');
        } catch (error) {
          console.error('Failed to process image:', error);
          alert(`OCR failed: ${error}`);
        }
      } else {
        console.log('❌ No image in clipboard');
        alert('No image found in clipboard. Please copy an image first (Win+Shift+S or Ctrl+C on an image).');
      }
    } catch (error) {
      console.error('❌ Failed to read clipboard:', error);
      alert(`Failed to read clipboard: ${error}`);
    } finally {
      setIsProcessing(false);
    }
  }, [onImageProcessed]);

  // Note: Ctrl+V is now handled globally in App-SplitView.tsx
  // No need for a local keyboard listener here

  // Handle file input
  const handleFileInput = (e: React.ChangeEvent<HTMLInputElement>) => {
    const file = e.target.files?.[0];
    if (file && file.type.startsWith('image/')) {
      processImageFile(file);
    }
  };

  const clearImage = () => {
    setUploadedImage(null);
    setProcessResult(null);
    if (fileInputRef.current) {
      fileInputRef.current.value = '';
    }
  };

  if (compact) {
    return (
      <div className="flex items-center gap-2">
        <input
          ref={fileInputRef}
          type="file"
          accept="image/*"
          onChange={handleFileInput}
          className="hidden"
        />
        <Button
          variant="outline"
          size="sm"
          onClick={handlePasteFromClipboard}
          disabled={isProcessing}
          title="Paste image from clipboard"
        >
          {isProcessing ? (
            <Loader2 className="h-4 w-4 animate-spin" />
          ) : (
            <Clipboard className="h-4 w-4" />
          )}
        </Button>
        <Button
          variant="outline"
          size="sm"
          onClick={() => fileInputRef.current?.click()}
          disabled={isProcessing}
          title="Choose image file"
        >
          <Upload className="h-4 w-4" />
        </Button>
      </div>
    );
  }

  return (
    <div className="space-y-4">
      {/* Drop Zone */}
      <div
        onDragOver={(e) => { e.preventDefault(); setIsDragging(true); }}
        onDragLeave={() => setIsDragging(false)}
        onDrop={handleDrop}
        className={`
          border-2 border-dashed rounded-lg p-8 transition-all
          ${isDragging ? 'border-blue-500 bg-blue-50 dark:bg-blue-950' : 'border-gray-300 dark:border-gray-700'}
          ${uploadedImage ? 'bg-gray-50 dark:bg-gray-900' : ''}
        `}
      >
        {uploadedImage ? (
          <div className="space-y-4">
            <div className="flex justify-between items-start">
              <div className="flex-1">
                <img
                  src={uploadedImage}
                  alt="Uploaded"
                  className="max-h-64 rounded-lg object-contain"
                />
              </div>
              <Button
                variant="ghost"
                size="sm"
                onClick={clearImage}
              >
                <X className="h-4 w-4" />
              </Button>
            </div>

            {isProcessing && (
              <div className="flex items-center gap-2 text-blue-600">
                <Loader2 className="h-4 w-4 animate-spin" />
                <span className="text-sm">Processing with OCR...</span>
              </div>
            )}

            {processResult && (
              <div className="space-y-2">
                <div className="flex items-center gap-2 text-green-600">
                  <CheckCircle2 className="h-4 w-4" />
                  <span className="text-sm font-medium">Processed Successfully</span>
                </div>
                <div className="text-sm space-y-1">
                  <p><strong>Extracted Text:</strong> {processResult.word_count} words</p>
                  <p><strong>Confidence:</strong> {(processResult.confidence * 100).toFixed(0)}%</p>
                  {processResult.extracted_text && (
                    <div className="mt-2 p-3 bg-white dark:bg-gray-800 rounded border">
                      <p className="text-xs text-gray-600 dark:text-gray-400 mb-1">Preview:</p>
                      <p className="text-sm line-clamp-3">{processResult.extracted_text}</p>
                    </div>
                  )}
                </div>
              </div>
            )}
          </div>
        ) : (
          <div className="text-center">
            <Upload className="h-12 w-12 mx-auto text-gray-400 mb-4" />
            <p className="text-lg font-medium text-gray-700 dark:text-gray-300 mb-2">
              Drop an image here or paste from clipboard
            </p>
            <p className="text-sm text-gray-500 mb-4">
              Copy an image to clipboard, then click "Paste from Clipboard"
            </p>
            <input
              ref={fileInputRef}
              type="file"
              accept="image/*"
              onChange={handleFileInput}
              className="hidden"
            />
            <div className="flex gap-2 justify-center">
              <Button
                variant="outline"
                onClick={() => fileInputRef.current?.click()}
              >
                <Upload className="h-4 w-4 mr-2" />
                Choose Image
              </Button>
              <Button
                variant="default"
                onClick={handlePasteFromClipboard}
                disabled={isProcessing}
              >
                {isProcessing ? (
                  <Loader2 className="h-4 w-4 mr-2 animate-spin" />
                ) : (
                  <Clipboard className="h-4 w-4 mr-2" />
                )}
                Paste from Clipboard
              </Button>
            </div>
          </div>
        )}
      </div>

      <p className="text-xs text-gray-500 text-center">
        Supports PNG, JPG, GIF • Text will be extracted using OCR and indexed for search
      </p>
    </div>
  );
}
