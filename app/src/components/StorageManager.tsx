import React, { useState, useEffect } from 'react';
import { invoke } from '@tauri-apps/api/core';
import {
  HardDrive, Trash2, FolderOpen, FileText, Database,
  AlertTriangle, CheckCircle, XCircle, RefreshCw,
  Zap, Archive, Download, Upload, Shield, Activity
} from 'lucide-react';
import { motion, AnimatePresence } from 'framer-motion';

interface StorageStats {
  totalDocuments: number;
  totalSpaces: number;
  databaseSizeMb: number;
  indexSizeMb: number;
  cacheSizeMb: number;
  lastBackup: string | null;
  health: 'healthy' | 'warning' | 'critical';
}

interface Space {
  id: string;
  name: string;
  emoji: string;
  documentCount: number;
  sizeMb: number;
  lastActive: string;
}

interface Document {
  id: string;
  title: string;
  spaceId: string;
  spaceName: string;
  sizekB: number;
  chunks: number;
  addedAt: string;
}

export function StorageManager() {
  const [stats, setStats] = useState<StorageStats | null>(null);
  const [spaces, setSpaces] = useState<Space[]>([]);
  const [documents, setDocuments] = useState<Document[]>([]);
  const [selectedSpace, setSelectedSpace] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [deleteMode, setDeleteMode] = useState(false);
  const [selectedDocs, setSelectedDocs] = useState<Set<string>>(new Set());
  const [showConfirm, setShowConfirm] = useState(false);
  const [operation, setOperation] = useState<'delete' | 'clear' | 'reset' | null>(null);

  useEffect(() => {
    loadStorageData();
  }, []);

  const loadStorageData = async () => {
    setLoading(true);
    try {
      const [storageStats, spaceList] = await Promise.all([
        invoke<StorageStats>('get_storage_stats'),
        invoke<Space[]>('get_spaces')
      ]);

      setStats(storageStats);
      setSpaces(spaceList);

      // Load documents for selected space or all
      if (selectedSpace) {
        const docs = await invoke<Document[]>('get_space_documents_detailed', {
          spaceId: selectedSpace
        });
        setDocuments(docs);
      }
    } catch (error) {
      console.error('Failed to load storage data:', error);
    } finally {
      setLoading(false);
    }
  };

  const handleDeleteDocuments = async () => {
    if (selectedDocs.size === 0) return;

    setShowConfirm(false);
    setLoading(true);

    try {
      const docIds = Array.from(selectedDocs);
      await invoke('delete_documents_batch', { documentIds: docIds });

      // Show success animation
      showSuccessAnimation();

      // Reload data
      await loadStorageData();
      setSelectedDocs(new Set());
      setDeleteMode(false);
    } catch (error) {
      console.error('Failed to delete documents:', error);
      showErrorAnimation();
    } finally {
      setLoading(false);
    }
  };

  const handleClearSpace = async (spaceId: string) => {
    setShowConfirm(false);
    setLoading(true);

    try {
      await invoke('clear_space_documents', { spaceId });
      showSuccessAnimation();
      await loadStorageData();
    } catch (error) {
      console.error('Failed to clear space:', error);
      showErrorAnimation();
    } finally {
      setLoading(false);
    }
  };

  const handleOptimizeStorage = async () => {
    setLoading(true);
    try {
      await invoke('optimize_storage');
      showSuccessAnimation('Storage optimized!');
      await loadStorageData();
    } catch (error) {
      console.error('Failed to optimize storage:', error);
    } finally {
      setLoading(false);
    }
  };

  const showSuccessAnimation = (message = 'Operation successful!') => {
    // This would trigger a toast or animation
    console.log(message);
  };

  const showErrorAnimation = () => {
    // This would trigger an error toast
    console.error('Operation failed');
  };

  const getHealthColor = (health: string) => {
    switch (health) {
      case 'healthy': return 'text-green-500';
      case 'warning': return 'text-yellow-500';
      case 'critical': return 'text-red-500';
      default: return 'text-gray-500';
    }
  };

  const getHealthIcon = (health: string) => {
    switch (health) {
      case 'healthy': return <CheckCircle className="w-5 h-5" />;
      case 'warning': return <AlertTriangle className="w-5 h-5" />;
      case 'critical': return <XCircle className="w-5 h-5" />;
      default: return <Activity className="w-5 h-5" />;
    }
  };

  const formatBytes = (bytes: number) => {
    if (bytes < 1024) return bytes + ' B';
    if (bytes < 1048576) return (bytes / 1024).toFixed(2) + ' KB';
    return (bytes / 1048576).toFixed(2) + ' MB';
  };

  return (
    <div className="p-6 max-w-7xl mx-auto">
      {/* Header */}
      <div className="mb-6">
        <h1 className="text-3xl font-bold text-gray-800 dark:text-white flex items-center gap-3">
          <HardDrive className="w-8 h-8 text-blue-500" />
          Storage Manager
        </h1>
        <p className="text-gray-600 dark:text-gray-400 mt-2">
          Manage your knowledge base storage and optimize performance
        </p>
      </div>

      {/* Storage Overview Cards */}
      {stats && (
        <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-4 gap-4 mb-6">
          <motion.div
            initial={{ opacity: 0, y: 20 }}
            animate={{ opacity: 1, y: 0 }}
            className="bg-white dark:bg-gray-800 rounded-lg shadow-lg p-4 border-l-4 border-blue-500"
          >
            <div className="flex items-center justify-between">
              <div>
                <p className="text-sm text-gray-600 dark:text-gray-400">Total Storage</p>
                <p className="text-2xl font-bold text-gray-900 dark:text-white">
                  {stats.databaseSizeMb.toFixed(2)} MB
                </p>
                <div className="mt-2 w-full bg-gray-200 rounded-full h-2">
                  <motion.div
                    className="bg-blue-500 h-2 rounded-full"
                    initial={{ width: 0 }}
                    animate={{ width: `${Math.min(stats.databaseSizeMb / 100, 100)}%` }}
                    transition={{ duration: 1 }}
                  />
                </div>
              </div>
              <Database className="w-8 h-8 text-blue-500 opacity-50" />
            </div>
          </motion.div>

          <motion.div
            initial={{ opacity: 0, y: 20 }}
            animate={{ opacity: 1, y: 0 }}
            transition={{ delay: 0.1 }}
            className="bg-white dark:bg-gray-800 rounded-lg shadow-lg p-4 border-l-4 border-green-500"
          >
            <div className="flex items-center justify-between">
              <div>
                <p className="text-sm text-gray-600 dark:text-gray-400">Documents</p>
                <p className="text-2xl font-bold text-gray-900 dark:text-white">
                  {stats.totalDocuments.toLocaleString()}
                </p>
                <p className="text-xs text-gray-500 mt-1">
                  Across {stats.totalSpaces} spaces
                </p>
              </div>
              <FileText className="w-8 h-8 text-green-500 opacity-50" />
            </div>
          </motion.div>

          <motion.div
            initial={{ opacity: 0, y: 20 }}
            animate={{ opacity: 1, y: 0 }}
            transition={{ delay: 0.2 }}
            className="bg-white dark:bg-gray-800 rounded-lg shadow-lg p-4 border-l-4 border-purple-500"
          >
            <div className="flex items-center justify-between">
              <div>
                <p className="text-sm text-gray-600 dark:text-gray-400">Index Size</p>
                <p className="text-2xl font-bold text-gray-900 dark:text-white">
                  {stats.indexSizeMb.toFixed(2)} MB
                </p>
                <p className="text-xs text-gray-500 mt-1">
                  Vector search index
                </p>
              </div>
              <Archive className="w-8 h-8 text-purple-500 opacity-50" />
            </div>
          </motion.div>

          <motion.div
            initial={{ opacity: 0, y: 20 }}
            animate={{ opacity: 1, y: 0 }}
            transition={{ delay: 0.3 }}
            className={`bg-white dark:bg-gray-800 rounded-lg shadow-lg p-4 border-l-4 ${
              stats.health === 'healthy' ? 'border-green-500' :
              stats.health === 'warning' ? 'border-yellow-500' : 'border-red-500'
            }`}
          >
            <div className="flex items-center justify-between">
              <div>
                <p className="text-sm text-gray-600 dark:text-gray-400">Health Status</p>
                <div className={`flex items-center gap-2 mt-2 ${getHealthColor(stats.health)}`}>
                  {getHealthIcon(stats.health)}
                  <span className="text-lg font-semibold capitalize">{stats.health}</span>
                </div>
              </div>
              <Shield className={`w-8 h-8 opacity-50 ${getHealthColor(stats.health)}`} />
            </div>
          </motion.div>
        </div>
      )}

      {/* Action Buttons */}
      <div className="flex gap-3 mb-6">
        <motion.button
          whileHover={{ scale: 1.05 }}
          whileTap={{ scale: 0.95 }}
          onClick={() => setDeleteMode(!deleteMode)}
          className={`px-4 py-2 rounded-lg font-medium transition-colors ${
            deleteMode
              ? 'bg-red-500 text-white hover:bg-red-600'
              : 'bg-gray-200 dark:bg-gray-700 text-gray-700 dark:text-gray-300 hover:bg-gray-300 dark:hover:bg-gray-600'
          }`}
        >
          <Trash2 className="w-4 h-4 inline mr-2" />
          {deleteMode ? 'Cancel Delete' : 'Delete Mode'}
        </motion.button>

        <motion.button
          whileHover={{ scale: 1.05 }}
          whileTap={{ scale: 0.95 }}
          onClick={handleOptimizeStorage}
          disabled={loading}
          className="px-4 py-2 bg-blue-500 text-white rounded-lg font-medium hover:bg-blue-600 disabled:opacity-50"
        >
          <Zap className="w-4 h-4 inline mr-2" />
          Optimize Storage
        </motion.button>

        <motion.button
          whileHover={{ scale: 1.05 }}
          whileTap={{ scale: 0.95 }}
          onClick={loadStorageData}
          disabled={loading}
          className="px-4 py-2 bg-gray-200 dark:bg-gray-700 text-gray-700 dark:text-gray-300 rounded-lg font-medium hover:bg-gray-300 dark:hover:bg-gray-600"
        >
          <RefreshCw className={`w-4 h-4 inline mr-2 ${loading ? 'animate-spin' : ''}`} />
          Refresh
        </motion.button>

        {deleteMode && selectedDocs.size > 0 && (
          <motion.button
            initial={{ opacity: 0, x: -20 }}
            animate={{ opacity: 1, x: 0 }}
            whileHover={{ scale: 1.05 }}
            whileTap={{ scale: 0.95 }}
            onClick={() => {
              setOperation('delete');
              setShowConfirm(true);
            }}
            className="px-4 py-2 bg-red-600 text-white rounded-lg font-medium hover:bg-red-700"
          >
            Delete {selectedDocs.size} Selected
          </motion.button>
        )}
      </div>

      {/* Spaces Grid */}
      <div className="grid grid-cols-1 lg:grid-cols-3 gap-6">
        {/* Spaces List */}
        <div className="lg:col-span-1">
          <h2 className="text-xl font-semibold mb-4 text-gray-800 dark:text-white">
            Spaces
          </h2>
          <div className="space-y-3">
            {spaces.map(space => (
              <motion.div
                key={space.id}
                whileHover={{ scale: 1.02 }}
                onClick={() => setSelectedSpace(space.id)}
                className={`p-4 rounded-lg cursor-pointer transition-all ${
                  selectedSpace === space.id
                    ? 'bg-blue-100 dark:bg-blue-900 border-2 border-blue-500'
                    : 'bg-white dark:bg-gray-800 border-2 border-transparent hover:border-gray-300 dark:hover:border-gray-600'
                } shadow`}
              >
                <div className="flex items-center justify-between">
                  <div className="flex items-center gap-3">
                    <span className="text-2xl">{space.emoji}</span>
                    <div>
                      <p className="font-medium text-gray-900 dark:text-white">
                        {space.name}
                      </p>
                      <p className="text-sm text-gray-600 dark:text-gray-400">
                        {space.documentCount} documents • {space.sizeMb.toFixed(2)} MB
                      </p>
                    </div>
                  </div>
                  {deleteMode && (
                    <motion.button
                      initial={{ opacity: 0 }}
                      animate={{ opacity: 1 }}
                      onClick={(e) => {
                        e.stopPropagation();
                        setOperation('clear');
                        setShowConfirm(true);
                      }}
                      className="p-2 bg-red-500 text-white rounded-lg hover:bg-red-600"
                    >
                      <Trash2 className="w-4 h-4" />
                    </motion.button>
                  )}
                </div>
              </motion.div>
            ))}
          </div>
        </div>

        {/* Documents List */}
        <div className="lg:col-span-2">
          <h2 className="text-xl font-semibold mb-4 text-gray-800 dark:text-white">
            Documents {selectedSpace && `in ${spaces.find(s => s.id === selectedSpace)?.name}`}
          </h2>

          {documents.length === 0 ? (
            <div className="text-center py-12 bg-white dark:bg-gray-800 rounded-lg">
              <FolderOpen className="w-16 h-16 mx-auto text-gray-400 mb-4" />
              <p className="text-gray-600 dark:text-gray-400">
                {selectedSpace ? 'No documents in this space' : 'Select a space to view documents'}
              </p>
            </div>
          ) : (
            <div className="bg-white dark:bg-gray-800 rounded-lg shadow overflow-hidden">
              <div className="overflow-x-auto">
                <table className="w-full">
                  <thead className="bg-gray-50 dark:bg-gray-700">
                    <tr>
                      {deleteMode && (
                        <th className="px-4 py-3">
                          <input
                            type="checkbox"
                            onChange={(e) => {
                              if (e.target.checked) {
                                setSelectedDocs(new Set(documents.map(d => d.id)));
                              } else {
                                setSelectedDocs(new Set());
                              }
                            }}
                            checked={selectedDocs.size === documents.length}
                            className="rounded"
                          />
                        </th>
                      )}
                      <th className="px-4 py-3 text-left text-sm font-medium text-gray-700 dark:text-gray-300">
                        Document
                      </th>
                      <th className="px-4 py-3 text-left text-sm font-medium text-gray-700 dark:text-gray-300">
                        Size
                      </th>
                      <th className="px-4 py-3 text-left text-sm font-medium text-gray-700 dark:text-gray-300">
                        Chunks
                      </th>
                      <th className="px-4 py-3 text-left text-sm font-medium text-gray-700 dark:text-gray-300">
                        Added
                      </th>
                    </tr>
                  </thead>
                  <tbody className="divide-y divide-gray-200 dark:divide-gray-700">
                    {documents.map(doc => (
                      <motion.tr
                        key={doc.id}
                        initial={{ opacity: 0 }}
                        animate={{ opacity: 1 }}
                        className={`hover:bg-gray-50 dark:hover:bg-gray-700 ${
                          selectedDocs.has(doc.id) ? 'bg-red-50 dark:bg-red-900' : ''
                        }`}
                      >
                        {deleteMode && (
                          <td className="px-4 py-3">
                            <input
                              type="checkbox"
                              checked={selectedDocs.has(doc.id)}
                              onChange={(e) => {
                                const newSelected = new Set(selectedDocs);
                                if (e.target.checked) {
                                  newSelected.add(doc.id);
                                } else {
                                  newSelected.delete(doc.id);
                                }
                                setSelectedDocs(newSelected);
                              }}
                              className="rounded"
                            />
                          </td>
                        )}
                        <td className="px-4 py-3">
                          <div>
                            <p className="font-medium text-gray-900 dark:text-white">
                              {doc.title}
                            </p>
                            <p className="text-xs text-gray-500">
                              {doc.spaceName}
                            </p>
                          </div>
                        </td>
                        <td className="px-4 py-3 text-sm text-gray-700 dark:text-gray-300">
                          {formatBytes(doc.sizekB * 1024)}
                        </td>
                        <td className="px-4 py-3 text-sm text-gray-700 dark:text-gray-300">
                          {doc.chunks}
                        </td>
                        <td className="px-4 py-3 text-sm text-gray-700 dark:text-gray-300">
                          {new Date(doc.addedAt).toLocaleDateString()}
                        </td>
                      </motion.tr>
                    ))}
                  </tbody>
                </table>
              </div>
            </div>
          )}
        </div>
      </div>

      {/* Confirmation Dialog */}
      <AnimatePresence>
        {showConfirm && (
          <motion.div
            initial={{ opacity: 0 }}
            animate={{ opacity: 1 }}
            exit={{ opacity: 0 }}
            className="fixed inset-0 bg-black bg-opacity-50 flex items-center justify-center z-50"
            onClick={() => setShowConfirm(false)}
          >
            <motion.div
              initial={{ scale: 0.9, opacity: 0 }}
              animate={{ scale: 1, opacity: 1 }}
              exit={{ scale: 0.9, opacity: 0 }}
              onClick={(e) => e.stopPropagation()}
              className="bg-white dark:bg-gray-800 rounded-lg shadow-xl p-6 max-w-md w-full mx-4"
            >
              <div className="flex items-center gap-3 mb-4">
                <AlertTriangle className="w-6 h-6 text-red-500" />
                <h3 className="text-xl font-semibold text-gray-900 dark:text-white">
                  Confirm {operation === 'delete' ? 'Deletion' : operation === 'clear' ? 'Clear Space' : 'Reset'}
                </h3>
              </div>
              <p className="text-gray-600 dark:text-gray-400 mb-6">
                {operation === 'delete'
                  ? `Are you sure you want to permanently delete ${selectedDocs.size} document(s)?`
                  : operation === 'clear'
                  ? 'Are you sure you want to clear all documents from this space?'
                  : 'Are you sure you want to reset the entire database?'}
              </p>
              <div className="flex gap-3 justify-end">
                <button
                  onClick={() => setShowConfirm(false)}
                  className="px-4 py-2 bg-gray-200 dark:bg-gray-700 text-gray-700 dark:text-gray-300 rounded-lg font-medium hover:bg-gray-300 dark:hover:bg-gray-600"
                >
                  Cancel
                </button>
                <button
                  onClick={() => {
                    if (operation === 'delete') {
                      handleDeleteDocuments();
                    } else if (operation === 'clear' && selectedSpace) {
                      handleClearSpace(selectedSpace);
                    }
                  }}
                  className="px-4 py-2 bg-red-600 text-white rounded-lg font-medium hover:bg-red-700"
                >
                  {operation === 'delete' ? 'Delete' : operation === 'clear' ? 'Clear' : 'Reset'}
                </button>
              </div>
            </motion.div>
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  );
}