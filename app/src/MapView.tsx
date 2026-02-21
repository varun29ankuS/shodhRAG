import { useEffect, useState, useRef } from 'react';
import { invoke } from '@tauri-apps/api/core';
import './MapView.css';

interface Document {
  id: string;
  title: string;
  content?: string;
  metadata?: Record<string, any>;
  similarity?: number;
}

interface Connection {
  source: string;
  target: string;
  strength: number;
}

interface Statistics {
  total_documents: number;
  total_vectors: number;
  vector_dimension: number;
  index_type: string;
}

export default function MapViewWindow() {
  const [documents, setDocuments] = useState<Document[]>([]);
  const [connections, setConnections] = useState<Connection[]>([]);
  const [selectedDoc, setSelectedDoc] = useState<Document | null>(null);
  const [loading, setLoading] = useState(true);
  const [stats, setStats] = useState<Statistics | null>(null);
  const [viewMode, setViewMode] = useState<'grid' | 'graph' | 'list'>('grid');
  const [searchQuery, setSearchQuery] = useState('');
  const [filteredDocs, setFilteredDocs] = useState<Document[]>([]);

  useEffect(() => {
    loadKnowledgeMap();
  }, []);

  useEffect(() => {
    if (searchQuery) {
      const filtered = documents.filter(doc => 
        doc.title.toLowerCase().includes(searchQuery.toLowerCase()) ||
        doc.content?.toLowerCase().includes(searchQuery.toLowerCase())
      );
      setFilteredDocs(filtered);
    } else {
      setFilteredDocs(documents);
    }
  }, [searchQuery, documents]);

  const loadKnowledgeMap = async () => {
    try {
      setLoading(true);

      // Load real documents from RAG system
      const docs = await invoke<Document[]>('list_space_documents', { spaceId: 'global' });
      setDocuments(docs);
      setFilteredDocs(docs);

      // Load statistics
      const statistics = await invoke<Statistics>('get_statistics');
      setStats(statistics);

      // Generate connections based on similarity (simplified for now)
      // In a real implementation, this would analyze vector similarities
      const conns: Connection[] = [];
      for (let i = 0; i < Math.min(docs.length, 10); i++) {
        for (let j = i + 1; j < Math.min(docs.length, 10); j++) {
          if (Math.random() > 0.6) { // Simplified - would use actual similarity
            conns.push({
              source: docs[i].id,
              target: docs[j].id,
              strength: Math.random()
            });
          }
        }
      }
      setConnections(conns);

    } catch (error) {
      console.error('Failed to load knowledge map:', error);
    } finally {
      setLoading(false);
    }
  };

  const getDocumentIcon = (doc: Document) => {
    const ext = doc.title.split('.').pop()?.toLowerCase();
    switch (ext) {
      case 'pdf': return '📕';
      case 'md': return '📝';
      case 'txt': return '📄';
      case 'py': return '🐍';
      case 'js': case 'ts': return '📜';
      case 'rs': return '🦀';
      default: return '📄';
    }
  };

  const getDocumentColor = (doc: Document) => {
    const ext = doc.title.split('.').pop()?.toLowerCase();
    switch (ext) {
      case 'pdf': return 'var(--error)';
      case 'md': return 'var(--accent)';
      case 'py': return 'var(--warning)';
      case 'js': case 'ts': return 'var(--success)';
      default: return 'var(--text-dim)';
    }
  };

  if (loading) {
    return (
      <div className="map-view-container">
        <div className="map-loading">
          <div className="map-spinner"></div>
          <p>Building knowledge map...</p>
        </div>
      </div>
    );
  }

  return (
    <div className="map-view-container">
      <div className="map-content">
        {/* Header with View Controls */}
        <div className="map-controls-bar">
          <div className="map-title-section">
            <h1>Knowledge Map</h1>
            <span className="map-subtitle">{stats?.total_documents || 0} documents connected</span>
          </div>

          <div className="map-search-box">
            <input
              type="text"
              placeholder="Search documents..."
              value={searchQuery}
              onChange={(e) => setSearchQuery(e.target.value)}
              className="map-search-input"
            />
          </div>

          <div className="view-mode-switcher">
            <button 
              className={`view-mode-btn ${viewMode === 'grid' ? 'active' : ''}`}
              onClick={() => setViewMode('grid')}
              title="Grid View"
            >
              ⚏
            </button>
            <button 
              className={`view-mode-btn ${viewMode === 'graph' ? 'active' : ''}`}
              onClick={() => setViewMode('graph')}
              title="Graph View"
            >
              ◉
            </button>
            <button 
              className={`view-mode-btn ${viewMode === 'list' ? 'active' : ''}`}
              onClick={() => setViewMode('list')}
              title="List View"
            >
              ☰
            </button>
          </div>
        </div>

        {/* Main Content Area */}
        <div className="map-main-content">
          {viewMode === 'grid' && (
            <div className="document-grid">
              {filteredDocs.length > 0 ? (
                filteredDocs.map((doc) => (
                  <div 
                    key={doc.id} 
                    className={`document-node ${selectedDoc?.id === doc.id ? 'selected' : ''}`}
                    onClick={() => setSelectedDoc(doc)}
                    style={{ borderColor: getDocumentColor(doc) }}
                  >
                    <div className="node-icon">{getDocumentIcon(doc)}</div>
                    <div className="node-title">{doc.title}</div>
                    <div className="node-meta">
                      {doc.metadata?.size ? `${(doc.metadata.size / 1024).toFixed(1)} KB` : ''}
                    </div>
                  </div>
                ))
              ) : (
                <div className="empty-map-state">
                  <p>No documents found</p>
                  <p className="hint">Add documents to see your knowledge map</p>
                </div>
              )}
            </div>
          )}

          {viewMode === 'graph' && (
            <div className="graph-view">
              <div className="graph-container">
                {/* Simplified graph visualization */}
                <svg className="graph-svg" viewBox="0 0 800 600">
                  {/* Draw connections */}
                  {connections.map((conn, idx) => {
                    const sourceDoc = documents.find(d => d.id === conn.source);
                    const targetDoc = documents.find(d => d.id === conn.target);
                    if (!sourceDoc || !targetDoc) return null;
                    
                    const sourceIdx = documents.indexOf(sourceDoc);
                    const targetIdx = documents.indexOf(targetDoc);
                    const x1 = 100 + (sourceIdx % 5) * 150;
                    const y1 = 100 + Math.floor(sourceIdx / 5) * 100;
                    const x2 = 100 + (targetIdx % 5) * 150;
                    const y2 = 100 + Math.floor(targetIdx / 5) * 100;
                    
                    return (
                      <line
                        key={idx}
                        x1={x1} y1={y1}
                        x2={x2} y2={y2}
                        stroke="var(--border)"
                        strokeWidth={conn.strength * 2}
                        opacity={0.3}
                      />
                    );
                  })}
                  
                  {/* Draw nodes */}
                  {filteredDocs.slice(0, 20).map((doc, idx) => {
                    const x = 100 + (idx % 5) * 150;
                    const y = 100 + Math.floor(idx / 5) * 100;
                    
                    return (
                      <g key={doc.id} onClick={() => setSelectedDoc(doc)}>
                        <circle
                          cx={x} cy={y} r="20"
                          fill="var(--surface)"
                          stroke={getDocumentColor(doc)}
                          strokeWidth="2"
                          className="graph-node"
                        />
                        <text x={x} y={y} textAnchor="middle" dy="0.3em" fontSize="20">
                          {getDocumentIcon(doc)}
                        </text>
                        <text x={x} y={y + 35} textAnchor="middle" fontSize="10" fill="var(--text-dim)">
                          {doc.title.length > 15 ? doc.title.substring(0, 15) + '...' : doc.title}
                        </text>
                      </g>
                    );
                  })}
                </svg>
              </div>
            </div>
          )}

          {viewMode === 'list' && (
            <div className="document-list">
              <div className="list-header">
                <span>Document</span>
                <span>Type</span>
                <span>Size</span>
                <span>Connections</span>
              </div>
              {filteredDocs.map((doc) => {
                const docConnections = connections.filter(
                  c => c.source === doc.id || c.target === doc.id
                ).length;
                
                return (
                  <div 
                    key={doc.id} 
                    className={`list-item ${selectedDoc?.id === doc.id ? 'selected' : ''}`}
                    onClick={() => setSelectedDoc(doc)}
                  >
                    <div className="list-item-name">
                      <span className="list-icon">{getDocumentIcon(doc)}</span>
                      <span>{doc.title}</span>
                    </div>
                    <span className="list-item-type">
                      {doc.title.split('.').pop()?.toUpperCase() || 'FILE'}
                    </span>
                    <span className="list-item-size">
                      {doc.metadata?.size ? `${(doc.metadata.size / 1024).toFixed(1)} KB` : '-'}
                    </span>
                    <span className="list-item-connections">
                      {docConnections}
                    </span>
                  </div>
                );
              })}
            </div>
          )}
        </div>

        {/* Selected Document Panel */}
        {selectedDoc && (
          <div className="document-details-panel">
            <div className="panel-header">
              <h3>{selectedDoc.title}</h3>
              <button 
                className="panel-close"
                onClick={() => setSelectedDoc(null)}
              >
                ×
              </button>
            </div>
            <div className="panel-content">
              <div className="detail-row">
                <span className="detail-label">Type</span>
                <span className="detail-value">
                  {selectedDoc.title.split('.').pop()?.toUpperCase() || 'FILE'}
                </span>
              </div>
              <div className="detail-row">
                <span className="detail-label">Size</span>
                <span className="detail-value">
                  {selectedDoc.metadata?.size ? `${(selectedDoc.metadata.size / 1024).toFixed(1)} KB` : 'Unknown'}
                </span>
              </div>
              <div className="detail-row">
                <span className="detail-label">Connections</span>
                <span className="detail-value">
                  {connections.filter(c => c.source === selectedDoc.id || c.target === selectedDoc.id).length}
                </span>
              </div>
              {selectedDoc.content && (
                <div className="detail-preview">
                  <span className="detail-label">Preview</span>
                  <p className="preview-text">
                    {selectedDoc.content.substring(0, 200)}...
                  </p>
                </div>
              )}
            </div>
            <div className="panel-actions">
              <button className="panel-action-btn">Open</button>
              <button className="panel-action-btn">Search Similar</button>
            </div>
          </div>
        )}

        {/* Stats Bar */}
        <div className="map-stats-bar">
          <div className="stat-item">
            <span className="stat-value">{stats?.total_documents || 0}</span>
            <span className="stat-label">Documents</span>
          </div>
          <div className="stat-item">
            <span className="stat-value">{stats?.total_vectors || 0}</span>
            <span className="stat-label">Vectors</span>
          </div>
          <div className="stat-item">
            <span className="stat-value">{connections.length}</span>
            <span className="stat-label">Connections</span>
          </div>
          <div className="stat-item">
            <span className="stat-value">{stats?.index_type || 'Vamana'}</span>
            <span className="stat-label">Index</span>
          </div>
        </div>
      </div>
    </div>
  );
}