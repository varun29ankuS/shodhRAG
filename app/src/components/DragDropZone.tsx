/**
 * Enhanced Drag & Drop Zone with Animations
 *
 * Supports:
 * - File uploads (documents, PDFs, images)
 * - Folder uploads
 * - Multiple file selection
 * - Visual feedback with Framer Motion
 * - Production-grade UX
 */

import React, { useCallback, useState } from 'react';
import { useDropzone, FileRejection } from 'react-dropzone';
import { motion, AnimatePresence } from 'framer-motion';
import {
  Upload,
  File,
  FileText,
  Image,
  Folder,
  CheckCircle,
  XCircle,
  Loader2
} from 'lucide-react';
import { invoke } from '@tauri-apps/api/core';

interface DragDropZoneProps {
  onFilesAccepted?: (files: File[]) => void;
  onUploadComplete?: (results: UploadResult[]) => void;
  acceptedFileTypes?: string[];
  maxFiles?: number;
  maxSize?: number; // in bytes
  spaceId?: string;
  autoUpload?: boolean;
  className?: string;
}

interface UploadResult {
  file: File;
  success: boolean;
  error?: string;
}

interface UploadingFile {
  file: File;
  progress: number;
  status: 'uploading' | 'success' | 'error';
  error?: string;
}

export const DragDropZone: React.FC<DragDropZoneProps> = ({
  onFilesAccepted,
  onUploadComplete,
  acceptedFileTypes = [
    '.pdf', '.doc', '.docx', '.txt', '.md',
    '.png', '.jpg', '.jpeg', '.gif', '.webp',
    '.json', '.csv', '.xlsx', '.pptx'
  ],
  maxFiles = 10,
  maxSize = 100 * 1024 * 1024, // 100MB default
  spaceId,
  autoUpload = true,
  className = ''
}) => {
  const [uploadingFiles, setUploadingFiles] = useState<UploadingFile[]>([]);
  const [isUploading, setIsUploading] = useState(false);
  const [draggedFileInfo, setDraggedFileInfo] = useState<{count: number, types: string[]} | null>(null);

  const uploadFile = async (file: File): Promise<UploadResult> => {
    try {
      // Update progress
      setUploadingFiles(prev => prev.map(uf =>
        uf.file.name === file.name
          ? { ...uf, progress: 30 }
          : uf
      ));

      // Read file as base64
      const reader = new FileReader();
      const fileData = await new Promise<string>((resolve, reject) => {
        reader.onload = () => resolve(reader.result as string);
        reader.onerror = reject;
        reader.readAsDataURL(file);
      });

      setUploadingFiles(prev => prev.map(uf =>
        uf.file.name === file.name
          ? { ...uf, progress: 60 }
          : uf
      ));

      // Upload to Tauri backend
      await invoke('upload_file', {
        fileName: file.name,
        fileData: fileData.split(',')[1], // Remove data URL prefix
        spaceId: spaceId || null,
      });

      setUploadingFiles(prev => prev.map(uf =>
        uf.file.name === file.name
          ? { ...uf, progress: 100, status: 'success' }
          : uf
      ));

      return { file, success: true };
    } catch (error) {
      const errorMessage = error instanceof Error ? error.message : 'Upload failed';

      setUploadingFiles(prev => prev.map(uf =>
        uf.file.name === file.name
          ? { ...uf, status: 'error', error: errorMessage }
          : uf
      ));

      return { file, success: false, error: errorMessage };
    }
  };

  const handleDrop = useCallback(async (acceptedFiles: File[], rejectedFiles: FileRejection[]) => {
    // Clear drag info on drop
    setDraggedFileInfo(null);

    console.log('📥 Files dropped:', {
      accepted: acceptedFiles.length,
      rejected: rejectedFiles.length
    });

    // Show rejected files
    if (rejectedFiles.length > 0) {
      rejectedFiles.forEach(({ file, errors }) => {
        console.error(`❌ Rejected: ${file.name}`, errors);
      });
    }

    if (acceptedFiles.length === 0) return;

    // Call parent handler
    onFilesAccepted?.(acceptedFiles);

    // Auto upload if enabled
    if (autoUpload) {
      setIsUploading(true);

      // Initialize uploading state
      setUploadingFiles(acceptedFiles.map(file => ({
        file,
        progress: 0,
        status: 'uploading'
      })));

      // Upload all files
      const results = await Promise.all(
        acceptedFiles.map(file => uploadFile(file))
      );

      setIsUploading(false);
      onUploadComplete?.(results);

      // Clear after 3 seconds
      setTimeout(() => {
        setUploadingFiles([]);
      }, 3000);
    }
  }, [autoUpload, onFilesAccepted, onUploadComplete, spaceId]);

  const {
    getRootProps,
    getInputProps,
    isDragActive,
    isDragAccept,
    isDragReject
  } = useDropzone({
    onDrop: handleDrop,
    onDragEnter: (event) => {
      // Detect file types being dragged
      const items = event.dataTransfer?.items;
      if (items) {
        const fileTypes: string[] = [];
        for (let i = 0; i < items.length; i++) {
          const item = items[i];
          if (item.kind === 'file') {
            // Try to get extension from type
            const type = item.type;
            let ext = '';
            if (type === 'application/pdf') ext = 'PDF';
            else if (type.includes('word') || type.includes('document')) ext = 'DOCX';
            else if (type.includes('presentation') || type.includes('powerpoint')) ext = 'PPTX';
            else if (type.includes('sheet') || type.includes('excel')) ext = 'XLSX';
            else if (type === 'text/csv') ext = 'CSV';
            else if (type === 'text/plain') ext = 'TXT';
            else if (type === 'text/markdown') ext = 'MD';
            else if (type.startsWith('image/')) ext = type.split('/')[1].toUpperCase();
            else if (type === 'application/json') ext = 'JSON';
            else ext = 'FILE';

            fileTypes.push(ext);
          }
        }
        setDraggedFileInfo({ count: items.length, types: [...new Set(fileTypes)] });
      }
    },
    onDragLeave: () => {
      setDraggedFileInfo(null);
    },
    accept: acceptedFileTypes.reduce((acc, type) => {
      // Convert extensions to MIME types
      const mimeTypes: Record<string, string[]> = {
        '.pdf': ['application/pdf'],
        '.doc': ['application/msword'],
        '.docx': ['application/vnd.openxmlformats-officedocument.wordprocessingml.document'],
        '.pptx': ['application/vnd.openxmlformats-officedocument.presentationml.presentation'],
        '.txt': ['text/plain'],
        '.md': ['text/markdown'],
        '.png': ['image/png'],
        '.jpg': ['image/jpeg'],
        '.jpeg': ['image/jpeg'],
        '.gif': ['image/gif'],
        '.webp': ['image/webp'],
        '.json': ['application/json'],
        '.csv': ['text/csv'],
        '.xlsx': ['application/vnd.openxmlformats-officedocument.spreadsheetml.sheet']
      };

      const mimes = mimeTypes[type] || [];
      mimes.forEach(mime => {
        if (!acc[mime]) acc[mime] = [];
      });

      return acc;
    }, {} as Record<string, string[]>),
    maxFiles,
    maxSize,
    multiple: maxFiles > 1
  });

  const getIcon = (fileName: string) => {
    const ext = fileName.toLowerCase().split('.').pop();

    if (['png', 'jpg', 'jpeg', 'gif', 'webp'].includes(ext || '')) {
      return <Image className="w-6 h-6" />;
    }
    if (['pdf', 'doc', 'docx'].includes(ext || '')) {
      return <FileText className="w-6 h-6" />;
    }
    return <File className="w-6 h-6" />;
  };

  return (
    <div className={`space-y-4 ${className}`}>
      {/* Drop Zone */}
      <motion.div
        {...getRootProps()}
        animate={{
          scale: isDragActive ? 1.02 : 1,
          borderColor: isDragAccept
            ? '#10b981'
            : isDragReject
            ? '#ef4444'
            : '#d1d5db',
          backgroundColor: isDragActive
            ? 'rgba(59, 130, 246, 0.05)'
            : 'transparent'
        }}
        transition={{ type: "spring", stiffness: 300, damping: 20 }}
        className="border-2 border-dashed rounded-xl p-8 cursor-pointer transition-colors"
      >
        <input {...getInputProps()} />

        <div className="flex flex-col items-center justify-center space-y-4 text-center">
          <AnimatePresence mode="wait">
            {isDragActive ? (
              <motion.div
                key="dragging"
                initial={{ scale: 0.8, opacity: 0 }}
                animate={{ scale: 1, opacity: 1 }}
                exit={{ scale: 0.8, opacity: 0 }}
                transition={{ type: "spring", stiffness: 400 }}
              >
                <motion.div
                  animate={{ y: [0, -10, 0] }}
                  transition={{ duration: 1, repeat: Infinity }}
                >
                  {isDragAccept ? (
                    <Upload className="w-16 h-16 text-green-500" />
                  ) : (
                    <XCircle className="w-16 h-16 text-red-500" />
                  )}
                </motion.div>
              </motion.div>
            ) : (
              <motion.div
                key="idle"
                initial={{ scale: 0.8, opacity: 0 }}
                animate={{ scale: 1, opacity: 1 }}
                exit={{ scale: 0.8, opacity: 0 }}
              >
                <Upload className="w-16 h-16 text-gray-400" />
              </motion.div>
            )}
          </AnimatePresence>

          <div>
            <p className="text-lg font-semibold text-gray-900 dark:text-white">
              {isDragActive
                ? isDragAccept
                  ? 'Drop files here'
                  : 'Some files will be rejected'
                : 'Drag & drop files here'
              }
            </p>
            <p className="text-sm text-gray-500 dark:text-gray-400 mt-1">
              or click to browse
            </p>

            {/* Show file type info during drag */}
            {draggedFileInfo && isDragActive && (
              <motion.div
                initial={{ opacity: 0, y: 10 }}
                animate={{ opacity: 1, y: 0 }}
                className="mt-3 flex items-center justify-center gap-2"
              >
                <div className="flex items-center gap-1 px-3 py-1 bg-blue-100 dark:bg-blue-900 rounded-full">
                  <span className="text-xs font-medium text-blue-900 dark:text-blue-100">
                    {draggedFileInfo.count} {draggedFileInfo.count === 1 ? 'file' : 'files'}
                  </span>
                  {draggedFileInfo.types.length > 0 && (
                    <>
                      <span className="text-xs text-blue-700 dark:text-blue-300 mx-1">•</span>
                      <span className="text-xs font-semibold text-blue-900 dark:text-blue-100">
                        {draggedFileInfo.types.join(', ')}
                      </span>
                    </>
                  )}
                </div>
              </motion.div>
            )}
          </div>

          <div className="text-xs text-gray-400 space-y-1">
            <p>Accepted: {acceptedFileTypes.slice(0, 5).join(', ')}
              {acceptedFileTypes.length > 5 && ` +${acceptedFileTypes.length - 5} more`}
            </p>
            <p>Max {maxFiles} files • Max {Math.round(maxSize / 1024 / 1024)}MB per file</p>
          </div>
        </div>
      </motion.div>

      {/* Uploading Files */}
      <AnimatePresence>
        {uploadingFiles.length > 0 && (
          <motion.div
            initial={{ opacity: 0, y: 20 }}
            animate={{ opacity: 1, y: 0 }}
            exit={{ opacity: 0, y: -20 }}
            className="space-y-2"
          >
            {uploadingFiles.map((uploadingFile, index) => (
              <motion.div
                key={uploadingFile.file.name}
                initial={{ opacity: 0, x: -20 }}
                animate={{ opacity: 1, x: 0 }}
                exit={{ opacity: 0, x: 20 }}
                transition={{ delay: index * 0.05 }}
                className="bg-white dark:bg-gray-800 rounded-lg p-4 shadow-sm border border-gray-200 dark:border-gray-700"
              >
                <div className="flex items-center justify-between">
                  <div className="flex items-center space-x-3 flex-1">
                    {getIcon(uploadingFile.file.name)}

                    <div className="flex-1 min-w-0">
                      <p className="text-sm font-medium text-gray-900 dark:text-white truncate">
                        {uploadingFile.file.name}
                      </p>
                      <p className="text-xs text-gray-500">
                        {(uploadingFile.file.size / 1024).toFixed(2)} KB
                      </p>
                    </div>
                  </div>

                  {/* Status Icon */}
                  <div className="ml-4">
                    {uploadingFile.status === 'uploading' && (
                      <Loader2 className="w-5 h-5 text-blue-500 animate-spin" />
                    )}
                    {uploadingFile.status === 'success' && (
                      <motion.div
                        initial={{ scale: 0 }}
                        animate={{ scale: 1 }}
                        transition={{ type: "spring", stiffness: 400 }}
                      >
                        <CheckCircle className="w-5 h-5 text-green-500" />
                      </motion.div>
                    )}
                    {uploadingFile.status === 'error' && (
                      <motion.div
                        initial={{ scale: 0 }}
                        animate={{ scale: 1 }}
                        transition={{ type: "spring", stiffness: 400 }}
                      >
                        <XCircle className="w-5 h-5 text-red-500" />
                      </motion.div>
                    )}
                  </div>
                </div>

                {/* Progress Bar */}
                {uploadingFile.status === 'uploading' && (
                  <motion.div
                    className="mt-2 h-1 bg-gray-200 dark:bg-gray-700 rounded-full overflow-hidden"
                  >
                    <motion.div
                      className="h-full bg-blue-500"
                      initial={{ width: 0 }}
                      animate={{ width: `${uploadingFile.progress}%` }}
                      transition={{ type: "spring", stiffness: 100 }}
                    />
                  </motion.div>
                )}

                {/* Error Message */}
                {uploadingFile.status === 'error' && uploadingFile.error && (
                  <motion.p
                    initial={{ opacity: 0, height: 0 }}
                    animate={{ opacity: 1, height: 'auto' }}
                    className="mt-2 text-xs text-red-500"
                  >
                    {uploadingFile.error}
                  </motion.p>
                )}
              </motion.div>
            ))}
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  );
};

export default DragDropZone;
