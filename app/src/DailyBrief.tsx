import { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import './DailyBrief.css';

interface Statistics {
  total_documents: number;
  total_vectors: number;
  vector_dimension: number;
  index_type: string;
  neural_rag_status: string;
  documents_today?: number;
  recent_queries?: string[];
}

interface Document {
  id: string;
  title: string;
  content?: string;
  timestamp?: string;
  metadata?: Record<string, any>;
}

export default function DailyBriefWindow() {
  const [stats, setStats] = useState<Statistics | null>(null);
  const [recentDocs, setRecentDocs] = useState<Document[]>([]);
  const [loading, setLoading] = useState(true);
  const [searchHistory, setSearchHistory] = useState<string[]>([]);

  useEffect(() => {
    loadDailyData();
    const interval = setInterval(loadDailyData, 30000);
    return () => clearInterval(interval);
  }, []);

  const loadDailyData = async () => {
    try {
      setLoading(true);
      
      // Get real statistics from RAG system
      const statistics = await invoke<Statistics>('get_statistics');
      setStats(statistics);

      // Get recent documents
      try {
        const docs = await invoke<Document[]>('list_space_documents', { spaceId: 'global' });
        setRecentDocs(docs.slice(0, 5));
      } catch (e) {
        console.error('Failed to load documents:', e);
      }

      // Load search history from localStorage
      const history = JSON.parse(localStorage.getItem('search_history') || '[]');
      setSearchHistory(history.slice(0, 10));

    } catch (error) {
      console.error('Failed to load daily brief data:', error);
    } finally {
      setLoading(false);
    }
  };

  const formatNumber = (num: number) => {
    return new Intl.NumberFormat().format(num);
  };

  const getTimeOfDay = () => {
    const hour = new Date().getHours();
    if (hour < 12) return 'Morning';
    if (hour < 17) return 'Afternoon';
    return 'Evening';
  };

  if (loading) {
    return (
      <div className="daily-brief-container">
        <div className="brief-loading">
          <div className="brief-spinner"></div>
          <p>Analyzing your knowledge base...</p>
        </div>
      </div>
    );
  }

  return (
    <div className="daily-brief-container">
      <div className="brief-content">
        {/* Header Section */}
        <div className="brief-welcome">
          <h1>Good {getTimeOfDay()}!</h1>
          <p className="brief-date">{new Date().toLocaleDateString('en-US', { 
            weekday: 'long', 
            year: 'numeric', 
            month: 'long', 
            day: 'numeric' 
          })}</p>
        </div>

        {/* Main Stats Grid */}
        <div className="brief-stats-grid">
          <div className="brief-stat-card primary">
            <div className="stat-content">
              <div className="stat-value">{formatNumber(stats?.total_documents || 0)}</div>
              <div className="stat-label">Total Documents</div>
            </div>
            <div className="stat-icon">📚</div>
          </div>

          <div className="brief-stat-card">
            <div className="stat-content">
              <div className="stat-value">{formatNumber(stats?.total_vectors || 0)}</div>
              <div className="stat-label">Vector Embeddings</div>
            </div>
            <div className="stat-icon">🔮</div>
          </div>

          <div className="brief-stat-card">
            <div className="stat-content">
              <div className="stat-value">{stats?.index_type || 'Vamana'}</div>
              <div className="stat-label">Index Type</div>
            </div>
            <div className="stat-icon">🗂️</div>
          </div>

          <div className="brief-stat-card accent">
            <div className="stat-content">
              <div className="stat-value">{stats?.neural_rag_status || 'Active'}</div>
              <div className="stat-label">Neural RAG</div>
            </div>
            <div className="stat-icon">🧠</div>
          </div>
        </div>

        {/* Two Column Layout */}
        <div className="brief-columns">
          {/* Recent Activity */}
          <div className="brief-section">
            <div className="section-header">
              <h2>Recent Activity</h2>
              <span className="section-badge">{searchHistory.length} searches</span>
            </div>
            
            {searchHistory.length > 0 ? (
              <div className="activity-list">
                {searchHistory.map((query, idx) => (
                  <div key={idx} className="activity-item">
                    <span className="activity-icon">🔍</span>
                    <span className="activity-text">{query}</span>
                  </div>
                ))}
              </div>
            ) : (
              <div className="empty-state">
                <p>No recent searches</p>
              </div>
            )}
          </div>

          {/* Recent Documents */}
          <div className="brief-section">
            <div className="section-header">
              <h2>Latest Documents</h2>
              <span className="section-badge">{recentDocs.length} files</span>
            </div>
            
            {recentDocs.length > 0 ? (
              <div className="docs-list">
                {recentDocs.map((doc) => (
                  <div key={doc.id} className="doc-card">
                    <div className="doc-icon">📄</div>
                    <div className="doc-info">
                      <div className="doc-title">{doc.title || 'Untitled'}</div>
                      <div className="doc-meta">
                        {doc.metadata?.size ? `${(doc.metadata.size / 1024).toFixed(1)} KB` : 'Unknown size'}
                      </div>
                    </div>
                  </div>
                ))}
              </div>
            ) : (
              <div className="empty-state">
                <p>No documents indexed yet</p>
                <p className="hint">Start by adding documents to your knowledge base</p>
              </div>
            )}
          </div>
        </div>

        {/* Quick Actions */}
        <div className="brief-actions">
          <h2>Quick Actions</h2>
          <div className="action-grid">
            <button className="brief-action-btn">
              <span className="action-icon">📁</span>
              <span>Add Documents</span>
            </button>
            <button className="brief-action-btn">
              <span className="action-icon">🔍</span>
              <span>Search</span>
            </button>
            <button className="brief-action-btn">
              <span className="action-icon">🔄</span>
              <span>Sync</span>
            </button>
            <button className="brief-action-btn">
              <span className="action-icon">⚙️</span>
              <span>Settings</span>
            </button>
          </div>
        </div>

        {/* System Status */}
        <div className="brief-status">
          <div className="status-item">
            <div className="status-indicator active"></div>
            <span>System Active</span>
          </div>
          <div className="status-item">
            <div className="status-indicator active"></div>
            <span>Index Healthy</span>
          </div>
          <div className="status-item">
            <div className="status-indicator active"></div>
            <span>Auto-sync Enabled</span>
          </div>
        </div>
      </div>
    </div>
  );
}