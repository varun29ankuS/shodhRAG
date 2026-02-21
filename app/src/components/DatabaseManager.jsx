import React, { useState, useEffect } from 'react';
import { invoke } from '@tauri-apps/api/core';

export default function DatabaseManager({ onSpacesReset }) {
  const [stats, setStats] = useState(null);
  const [loading, setLoading] = useState(false);
  const [message, setMessage] = useState('');
  const [error, setError] = useState('');

  useEffect(() => {
    loadStats();
  }, []);

  const loadStats = async () => {
    try {
      const dbStats = await invoke('get_database_stats');
      setStats(dbStats);
    } catch (err) {
      console.error('Failed to load stats:', err);
    }
  };

  const handleAction = async (action, confirmMessage) => {
    if (!confirm(confirmMessage)) return;

    setLoading(true);
    setMessage('');
    setError('');

    try {
      let result;
      switch (action) {
        case 'reset':
          result = await invoke('reset_database');
          setMessage('Database reset complete. Please restart the application.');
          if (onSpacesReset) onSpacesReset();
          break;
        case 'clear_docs':
          result = await invoke('clear_all_documents');
          setMessage('All documents cleared successfully.');
          break;
        case 'cleanup':
          result = await invoke('cleanup_orphaned_documents');
          setMessage(result || 'Cleanup complete.');
          break;
        default:
          throw new Error('Unknown action');
      }

      // Reload stats after action
      await loadStats();
    } catch (err) {
      setError(`Action failed: ${err}`);
    } finally {
      setLoading(false);
    }
  };

  const formatSize = (mb) => {
    if (mb < 1) return `${(mb * 1024).toFixed(1)} KB`;
    if (mb > 1024) return `${(mb / 1024).toFixed(2)} GB`;
    return `${mb.toFixed(2)} MB`;
  };

  return (
    <div className="database-manager" style={{ padding: '20px', backgroundColor: '#f5f5f5', borderRadius: '8px' }}>
      <div className="header" style={{ marginBottom: '20px' }}>
        <p style={{ color: '#666', marginTop: '8px' }}>
          Clear data and perform maintenance tasks
        </p>
      </div>

      {/* Statistics */}
      {stats && (
        <div className="stats-grid" style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fit, minmax(150px, 1fr))', gap: '15px', marginBottom: '20px' }}>
          <div className="stat-card" style={{ backgroundColor: 'white', padding: '15px', borderRadius: '8px', boxShadow: '0 2px 4px rgba(0,0,0,0.1)' }}>
            <div style={{ fontSize: '24px', fontWeight: 'bold' }}>📊 {stats.totalSpaces}</div>
            <div style={{ fontSize: '14px', color: '#666' }}>Spaces</div>
          </div>

          <div className="stat-card" style={{ backgroundColor: 'white', padding: '15px', borderRadius: '8px', boxShadow: '0 2px 4px rgba(0,0,0,0.1)' }}>
            <div style={{ fontSize: '24px', fontWeight: 'bold' }}>📄 {stats.totalDocuments}</div>
            <div style={{ fontSize: '14px', color: '#666' }}>Documents</div>
          </div>

          <div className="stat-card" style={{ backgroundColor: 'white', padding: '15px', borderRadius: '8px', boxShadow: '0 2px 4px rgba(0,0,0,0.1)' }}>
            <div style={{ fontSize: '24px', fontWeight: 'bold' }}>🔢 {stats.totalVectors}</div>
            <div style={{ fontSize: '14px', color: '#666' }}>Vectors</div>
          </div>

          <div className="stat-card" style={{ backgroundColor: 'white', padding: '15px', borderRadius: '8px', boxShadow: '0 2px 4px rgba(0,0,0,0.1)' }}>
            <div style={{ fontSize: '24px', fontWeight: 'bold' }}>💾 {formatSize(stats.databaseSizeMb)}</div>
            <div style={{ fontSize: '14px', color: '#666' }}>Database Size</div>
          </div>
        </div>
      )}

      {/* Actions */}
      <div className="actions">
        <div className="action-card" style={{ backgroundColor: 'white', padding: '20px', borderRadius: '8px', boxShadow: '0 2px 4px rgba(0,0,0,0.1)', marginBottom: '15px' }}>
          <h3 style={{ fontSize: '18px', fontWeight: '600', marginBottom: '15px' }}>Maintenance Actions</h3>

          <div style={{ display: 'flex', flexDirection: 'column', gap: '10px' }}>
            <button
              onClick={() => handleAction('cleanup',
                'This will remove documents not associated with any space. Continue?')}
              disabled={loading}
              style={{
                padding: '10px 20px',
                backgroundColor: loading ? '#ccc' : '#3b82f6',
                color: 'white',
                border: 'none',
                borderRadius: '6px',
                cursor: loading ? 'not-allowed' : 'pointer',
                fontSize: '14px',
                fontWeight: '500',
                display: 'flex',
                alignItems: 'center',
                gap: '8px'
              }}
            >
              {loading ? '⟳' : '🔄'} Clean Up Orphaned Documents
            </button>

            <button
              onClick={() => handleAction('clear_docs',
                'This will DELETE all documents but keep spaces. This action cannot be undone. Continue?')}
              disabled={loading}
              style={{
                padding: '10px 20px',
                backgroundColor: loading ? '#ccc' : '#f97316',
                color: 'white',
                border: 'none',
                borderRadius: '6px',
                cursor: loading ? 'not-allowed' : 'pointer',
                fontSize: '14px',
                fontWeight: '500',
                display: 'flex',
                alignItems: 'center',
                gap: '8px'
              }}
            >
              🗑️ Clear All Documents
            </button>

            <button
              onClick={() => handleAction('reset',
                '⚠️ WARNING: This will DELETE everything - all spaces, documents, and vectors. The app will need to be restarted. This CANNOT be undone. Are you absolutely sure?')}
              disabled={loading}
              style={{
                padding: '10px 20px',
                backgroundColor: loading ? '#ccc' : '#ef4444',
                color: 'white',
                border: 'none',
                borderRadius: '6px',
                cursor: loading ? 'not-allowed' : 'pointer',
                fontSize: '14px',
                fontWeight: '500',
                display: 'flex',
                alignItems: 'center',
                gap: '8px'
              }}
            >
              ⚠️ Reset Entire Database
            </button>
          </div>
        </div>

        {/* Messages */}
        {message && (
          <div style={{
            backgroundColor: '#dcfce7',
            border: '1px solid #86efac',
            color: '#166534',
            padding: '15px',
            borderRadius: '6px',
            marginBottom: '15px',
            display: 'flex',
            alignItems: 'start',
            gap: '8px'
          }}>
            ✅ <span>{message}</span>
          </div>
        )}

        {error && (
          <div style={{
            backgroundColor: '#fee2e2',
            border: '1px solid #fca5a5',
            color: '#991b1b',
            padding: '15px',
            borderRadius: '6px',
            marginBottom: '15px',
            display: 'flex',
            alignItems: 'start',
            gap: '8px'
          }}>
            ❌ <span>{error}</span>
          </div>
        )}

        <div style={{
          backgroundColor: '#dbeafe',
          border: '1px solid #93c5fd',
          padding: '15px',
          borderRadius: '6px'
        }}>
          <h4 style={{ fontWeight: '600', color: '#1e3a8a', marginBottom: '8px' }}>ℹ️ Information</h4>
          <ul style={{ fontSize: '14px', color: '#1e40af', listStyle: 'none', padding: 0 }}>
            <li>• Spaces are saved in: <code style={{ backgroundColor: '#bfdbfe', padding: '2px 4px', borderRadius: '3px' }}>./data/spaces.json</code></li>
            <li>• Database is stored in: <code style={{ backgroundColor: '#bfdbfe', padding: '2px 4px', borderRadius: '3px' }}>./kalki_data/rocksdb/</code></li>
            <li>• After resetting, restart the application for a fresh start</li>
            <li>• Regular cleanup helps maintain optimal performance</li>
          </ul>
        </div>
      </div>
    </div>
  );
}