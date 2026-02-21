import React, { useState, useEffect, useRef, useCallback, useMemo, lazy, Suspense } from 'react';
import { invoke } from '@tauri-apps/api/core';
import ForceGraph2D from 'react-force-graph-2d';
import type { NodeObject, LinkObject, ForceGraph2DMethods } from 'react-force-graph-2d';
import {
  RefreshCw,
  ZoomIn,
  ZoomOut,
  Maximize2,
  X,
  Filter,
  AlertCircle,
  Box,
  Layers,
} from 'lucide-react';
import { notify } from '../lib/notify';
import { useTheme } from '../contexts/ThemeContext';

const ForceGraph3D = lazy(() => import('react-force-graph-3d'));

interface GraphNode {
  id: string;
  label: string;
  type: string;
  size: number;
  color: string;
  metadata?: {
    documentCount?: number;
    connections?: number;
    score?: number;
    space?: string;
    filePath?: string;
  };
}

interface GraphEdge {
  source: string;
  target: string;
  weight: number;
  type: string;
  label?: string;
}

interface BackendGraphData {
  nodes: GraphNode[];
  edges: GraphEdge[];
}

type NodeFilter = 'all' | 'document' | 'topic' | 'entity';
type ViewMode = '2d' | '3d';

export default function KnowledgeGraph() {
  const { colors } = useTheme();
  const fgRef = useRef<ForceGraph2DMethods | null>(null);
  const fg3dRef = useRef<any>(null);
  const containerRef = useRef<HTMLDivElement>(null);

  const [graphData, setGraphData] = useState<{ nodes: NodeObject[]; links: LinkObject[] }>({ nodes: [], links: [] });
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [selectedNode, setSelectedNode] = useState<NodeObject | null>(null);
  const [hoveredNode, setHoveredNode] = useState<NodeObject | null>(null);
  const [tooltipPos, setTooltipPos] = useState<{ x: number; y: number }>({ x: 0, y: 0 });
  const [filter, setFilter] = useState<NodeFilter>('all');
  const [maxNodes, setMaxNodes] = useState(100);
  const [dimensions, setDimensions] = useState({ width: 800, height: 600 });
  const [viewMode, setViewMode] = useState<ViewMode>('2d');

  const fetchGraph = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const data = await invoke<BackendGraphData>('get_knowledge_graph', {
        spaceId: null,
        maxNodes,
        minSimilarity: 0.3,
      });

      const nodes: NodeObject[] = data.nodes.map(n => ({
        id: n.id,
        label: n.label,
        nodeType: n.type,
        size: n.size,
        color: n.color,
        metadata: n.metadata,
      }));

      const nodeIds = new Set(nodes.map(n => n.id));
      const links: LinkObject[] = data.edges
        .filter(e => nodeIds.has(e.source) && nodeIds.has(e.target))
        .map(e => ({
          source: e.source,
          target: e.target,
          weight: e.weight,
          edgeType: e.type,
          label: e.label,
        }));

      setGraphData({ nodes, links });
    } catch (err) {
      setError(`Failed to load graph: ${err}`);
      notify.error('Failed to load knowledge graph');
    } finally {
      setLoading(false);
    }
  }, [maxNodes]);

  useEffect(() => {
    fetchGraph();
  }, [fetchGraph]);

  // Observe container size
  useEffect(() => {
    const el = containerRef.current;
    if (!el) return;
    const ro = new ResizeObserver(entries => {
      const { width, height } = entries[0].contentRect;
      if (width > 0 && height > 0) {
        setDimensions({ width, height });
      }
    });
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  // Fit graph after data loads
  useEffect(() => {
    if (!loading && graphData.nodes.length > 0) {
      setTimeout(() => {
        if (viewMode === '2d') {
          fgRef.current?.zoomToFit(400, 60);
        } else {
          fg3dRef.current?.zoomToFit(400, 60);
        }
      }, 300);
    }
  }, [loading, graphData.nodes.length, viewMode]);

  const filteredData = useMemo(() => {
    if (filter === 'all') return graphData;
    const visibleNodes = graphData.nodes.filter(n => n.nodeType === filter);
    const visibleIds = new Set(visibleNodes.map(n => n.id));
    const visibleLinks = graphData.links.filter(l => {
      const srcId = typeof l.source === 'object' ? (l.source as NodeObject).id : l.source;
      const tgtId = typeof l.target === 'object' ? (l.target as NodeObject).id : l.target;
      return visibleIds.has(srcId as string) && visibleIds.has(tgtId as string);
    });
    return { nodes: visibleNodes, links: visibleLinks };
  }, [graphData, filter]);

  const stats = useMemo(() => {
    const docs = graphData.nodes.filter(n => n.nodeType === 'document').length;
    const topics = graphData.nodes.filter(n => n.nodeType === 'topic').length;
    const entities = graphData.nodes.filter(n => n.nodeType === 'entity').length;
    return { docs, topics, entities, edges: graphData.links.length };
  }, [graphData]);

  const nodeCanvasObject = useCallback((node: NodeObject, ctx: CanvasRenderingContext2D, globalScale: number) => {
    const label = node.label as string || '';
    const size = (node.size as number) || 4;
    const color = node.color as string || '#888';
    const x = node.x ?? 0;
    const y = node.y ?? 0;
    const isSelected = selectedNode?.id === node.id;
    const isHovered = hoveredNode?.id === node.id;
    const radius = size * (isSelected ? 1.4 : isHovered ? 1.2 : 1);

    // Glow for selected
    if (isSelected) {
      ctx.beginPath();
      ctx.arc(x, y, radius + 3, 0, 2 * Math.PI);
      ctx.fillStyle = `${color}30`;
      ctx.fill();
    }

    // Node circle
    ctx.beginPath();
    ctx.arc(x, y, radius, 0, 2 * Math.PI);
    ctx.fillStyle = color;
    ctx.fill();

    if (isSelected || isHovered) {
      ctx.strokeStyle = '#fff';
      ctx.lineWidth = 1.5 / globalScale;
      ctx.stroke();
    }

    // Label when zoomed in enough
    if (globalScale > 1.2 || isSelected || isHovered) {
      const fontSize = Math.max(10 / globalScale, 2);
      ctx.font = `${isSelected ? 'bold ' : ''}${fontSize}px Inter, system-ui, sans-serif`;
      ctx.textAlign = 'center';
      ctx.textBaseline = 'top';
      ctx.fillStyle = isSelected ? '#fff' : colors.text;
      const truncated = label.length > 24 ? label.slice(0, 22) + '…' : label;
      ctx.fillText(truncated, x, y + radius + 2);
    }
  }, [selectedNode, hoveredNode, colors.text]);

  // 3D node rendering via SpriteText
  const nodeThreeObject = useCallback((node: NodeObject) => {
    // Dynamically import to avoid loading three.js in 2D mode
    const SpriteText = require('three-spritetext').default;
    const label = (node.label as string) || '';
    const truncated = label.length > 20 ? label.slice(0, 18) + '…' : label;
    const sprite = new SpriteText(truncated);
    sprite.color = node.color as string || '#888';
    sprite.textHeight = 2;
    sprite.fontFace = 'Inter, system-ui, sans-serif';
    sprite.backgroundColor = 'rgba(0,0,0,0.4)';
    sprite.padding = 1;
    sprite.borderRadius = 2;
    return sprite;
  }, []);

  const linkColor = useCallback((link: LinkObject) => {
    const type = link.edgeType as string;
    switch (type) {
      case 'same_directory': return `${colors.success}50`;
      case 'semantic': return `${colors.primary}40`;
      case 'category': return `${colors.secondary}40`;
      case 'language': return '#f59e0b40';
      default: return `${colors.border}60`;
    }
  }, [colors]);

  const linkWidth = useCallback((link: LinkObject) => {
    return Math.max((link.weight as number || 0.3) * 2, 0.5);
  }, []);

  const handleNodeClick = useCallback((node: NodeObject) => {
    setSelectedNode(prev => prev?.id === node.id ? null : node);
  }, []);

  const handleNodeHover = useCallback((node: NodeObject | null, _prev: NodeObject | null) => {
    setHoveredNode(node);
    if (node && containerRef.current) {
      const rect = containerRef.current.getBoundingClientRect();
      if (viewMode === '2d') {
        const coords = fgRef.current?.graph2ScreenCoords(node.x ?? 0, node.y ?? 0);
        if (coords) {
          setTooltipPos({ x: coords.x + rect.left + 12, y: coords.y + rect.top - 8 });
        }
      } else {
        const coords = fg3dRef.current?.graph2ScreenCoords(node.x ?? 0, node.y ?? 0, (node as any).z ?? 0);
        if (coords) {
          setTooltipPos({ x: coords.x + rect.left + 12, y: coords.y + rect.top - 8 });
        }
      }
    }
  }, [viewMode]);

  const handleBackgroundClick = useCallback(() => {
    setSelectedNode(null);
  }, []);

  const handleZoomIn = useCallback(() => {
    if (viewMode === '2d') {
      fgRef.current?.zoom(((fgRef.current as any)?.__zoom?.k ?? 1) * 1.3, 200);
    }
    // 3D zoom is handled by scroll/orbit controls
  }, [viewMode]);

  const handleZoomOut = useCallback(() => {
    if (viewMode === '2d') {
      fgRef.current?.zoom(((fgRef.current as any)?.__zoom?.k ?? 1) * 0.7, 200);
    }
  }, [viewMode]);

  const handleFit = useCallback(() => {
    if (viewMode === '2d') {
      fgRef.current?.zoomToFit(400, 60);
    } else {
      fg3dRef.current?.zoomToFit(400, 60);
    }
  }, [viewMode]);

  if (loading) {
    return (
      <div className="h-full flex items-center justify-center">
        <div className="text-center space-y-3">
          <div
            className="w-8 h-8 border-2 border-t-transparent rounded-full animate-spin mx-auto"
            style={{ borderColor: colors.primary }}
          />
          <p className="text-sm" style={{ color: colors.textMuted }}>Building knowledge graph...</p>
        </div>
      </div>
    );
  }

  if (error) {
    return (
      <div className="h-full flex items-center justify-center">
        <div className="text-center space-y-3">
          <AlertCircle className="w-8 h-8 mx-auto" style={{ color: colors.error }} />
          <p className="text-sm" style={{ color: colors.textSecondary }}>{error}</p>
          <button
            onClick={fetchGraph}
            className="text-xs px-3 py-1.5 rounded-md border transition-colors"
            style={{ borderColor: colors.border, color: colors.textSecondary }}
          >
            <RefreshCw className="w-3 h-3 inline mr-1.5" />
            Retry
          </button>
        </div>
      </div>
    );
  }

  if (graphData.nodes.length === 0) {
    return (
      <div className="h-full flex items-center justify-center">
        <div className="text-center space-y-3">
          <div className="w-12 h-12 rounded-full mx-auto flex items-center justify-center" style={{ backgroundColor: `${colors.primary}14` }}>
            <svg className="w-6 h-6" viewBox="0 0 24 24" fill="none" stroke={colors.primary} strokeWidth="2">
              <circle cx="12" cy="12" r="3" />
              <circle cx="5" cy="6" r="2" />
              <circle cx="19" cy="6" r="2" />
              <circle cx="5" cy="18" r="2" />
              <circle cx="19" cy="18" r="2" />
              <line x1="9.5" y1="10.5" x2="6.5" y2="7.5" />
              <line x1="14.5" y1="10.5" x2="17.5" y2="7.5" />
              <line x1="9.5" y1="13.5" x2="6.5" y2="16.5" />
              <line x1="14.5" y1="13.5" x2="17.5" y2="16.5" />
            </svg>
          </div>
          <p className="text-sm font-medium" style={{ color: colors.text }}>No graph data</p>
          <p className="text-xs" style={{ color: colors.textMuted }}>Index some documents to see relationships</p>
        </div>
      </div>
    );
  }

  return (
    <div className="h-full flex flex-col">
      {/* Controls bar */}
      <div
        className="flex items-center justify-between px-4 py-2 border-b shrink-0"
        style={{ borderColor: colors.border }}
      >
        <div className="flex items-center gap-3">
          <h2 className="text-sm font-semibold" style={{ color: colors.text }}>Knowledge Graph</h2>
          <div className="flex items-center gap-1.5 text-[10px]" style={{ color: colors.textMuted }}>
            <span style={{ color: '#10b981' }}>{stats.docs} docs</span>
            <span>·</span>
            <span style={{ color: '#6366f1' }}>{stats.topics} topics</span>
            <span>·</span>
            <span style={{ color: '#f59e0b' }}>{stats.entities} entities</span>
            <span>·</span>
            <span>{stats.edges} edges</span>
          </div>
        </div>

        <div className="flex items-center gap-2">
          {/* 2D / 3D toggle */}
          <div className="flex items-center rounded-md border overflow-hidden" style={{ borderColor: colors.border }}>
            <button
              onClick={() => setViewMode('2d')}
              className="px-2 py-1 text-[10px] font-medium flex items-center gap-1 transition-colors"
              style={{
                backgroundColor: viewMode === '2d' ? `${colors.primary}18` : 'transparent',
                color: viewMode === '2d' ? colors.primary : colors.textMuted,
              }}
              title="2D view"
            >
              <Layers className="w-3 h-3" />
              2D
            </button>
            <button
              onClick={() => setViewMode('3d')}
              className="px-2 py-1 text-[10px] font-medium flex items-center gap-1 transition-colors"
              style={{
                backgroundColor: viewMode === '3d' ? `${colors.primary}18` : 'transparent',
                color: viewMode === '3d' ? colors.primary : colors.textMuted,
              }}
              title="3D view"
            >
              <Box className="w-3 h-3" />
              3D
            </button>
          </div>

          {/* Filter */}
          <div className="flex items-center gap-1">
            <Filter className="w-3 h-3" style={{ color: colors.textMuted }} />
            <select
              value={filter}
              onChange={e => setFilter(e.target.value as NodeFilter)}
              className="text-[10px] bg-transparent border rounded px-1.5 py-0.5 outline-none"
              style={{ borderColor: colors.border, color: colors.textSecondary }}
            >
              <option value="all">All Nodes</option>
              <option value="document">Documents</option>
              <option value="topic">Topics</option>
              <option value="entity">Entities</option>
            </select>
          </div>

          {/* Max nodes */}
          <select
            value={maxNodes}
            onChange={e => setMaxNodes(Number(e.target.value))}
            className="text-[10px] bg-transparent border rounded px-1.5 py-0.5 outline-none"
            style={{ borderColor: colors.border, color: colors.textSecondary }}
          >
            <option value={50}>50 nodes</option>
            <option value={100}>100 nodes</option>
            <option value={200}>200 nodes</option>
          </select>

          {/* Zoom controls */}
          <div className="flex items-center gap-0.5">
            <button
              onClick={handleZoomIn}
              className="w-6 h-6 rounded flex items-center justify-center border transition-colors"
              style={{ borderColor: colors.border, color: colors.textTertiary }}
              title="Zoom in"
            >
              <ZoomIn className="w-3 h-3" />
            </button>
            <button
              onClick={handleZoomOut}
              className="w-6 h-6 rounded flex items-center justify-center border transition-colors"
              style={{ borderColor: colors.border, color: colors.textTertiary }}
              title="Zoom out"
            >
              <ZoomOut className="w-3 h-3" />
            </button>
            <button
              onClick={handleFit}
              className="w-6 h-6 rounded flex items-center justify-center border transition-colors"
              style={{ borderColor: colors.border, color: colors.textTertiary }}
              title="Fit to view"
            >
              <Maximize2 className="w-3 h-3" />
            </button>
          </div>

          <button
            onClick={fetchGraph}
            className="w-6 h-6 rounded flex items-center justify-center border transition-colors"
            style={{ borderColor: colors.border, color: colors.textTertiary }}
            title="Refresh graph"
          >
            <RefreshCw className="w-3 h-3" />
          </button>
        </div>
      </div>

      {/* Graph canvas */}
      <div ref={containerRef} className="flex-1 relative overflow-hidden">
        {viewMode === '2d' ? (
          <ForceGraph2D
            ref={fgRef}
            graphData={filteredData}
            width={dimensions.width}
            height={dimensions.height}
            backgroundColor={colors.bg}
            nodeId="id"
            nodeCanvasObject={nodeCanvasObject}
            nodeCanvasObjectMode={() => 'replace'}
            nodeVal={(node: NodeObject) => (node.size as number) || 4}
            linkColor={linkColor}
            linkWidth={linkWidth}
            linkCurvature={0.1}
            onNodeClick={handleNodeClick}
            onNodeHover={handleNodeHover}
            onBackgroundClick={handleBackgroundClick}
            cooldownTicks={80}
            d3AlphaDecay={0.02}
            d3VelocityDecay={0.3}
            enableZoomInteraction={true}
            enablePanInteraction={true}
            enableNodeDrag={true}
            minZoom={0.3}
            maxZoom={10}
          />
        ) : (
          <Suspense fallback={
            <div className="h-full flex items-center justify-center">
              <div className="text-center space-y-2">
                <div className="w-6 h-6 border-2 border-t-transparent rounded-full animate-spin mx-auto" style={{ borderColor: colors.primary }} />
                <p className="text-xs" style={{ color: colors.textMuted }}>Loading 3D renderer...</p>
              </div>
            </div>
          }>
            <ForceGraph3D
              ref={fg3dRef}
              graphData={filteredData}
              width={dimensions.width}
              height={dimensions.height}
              backgroundColor={colors.bg}
              nodeId="id"
              nodeThreeObject={nodeThreeObject}
              nodeVal={(node: NodeObject) => (node.size as number) || 4}
              nodeColor={(node: NodeObject) => node.color as string || '#888'}
              linkColor={linkColor}
              linkWidth={linkWidth}
              linkCurvature={0.1}
              onNodeClick={handleNodeClick}
              onNodeHover={handleNodeHover}
              onBackgroundClick={handleBackgroundClick}
              cooldownTicks={80}
              d3AlphaDecay={0.02}
              d3VelocityDecay={0.3}
              enableNavigationControls={true}
              enableNodeDrag={true}
            />
          </Suspense>
        )}

        {/* Legend */}
        <div
          className="absolute bottom-4 left-4 flex items-center gap-3 px-3 py-2 rounded-lg border"
          style={{
            backgroundColor: `${colors.bgSecondary}e0`,
            borderColor: colors.border,
            backdropFilter: 'blur(8px)',
          }}
        >
          {[
            { color: '#10b981', label: 'Document' },
            { color: '#6366f1', label: 'Topic' },
            { color: '#f59e0b', label: 'Entity' },
          ].map(item => (
            <div key={item.label} className="flex items-center gap-1.5">
              <div className="w-2.5 h-2.5 rounded-full" style={{ backgroundColor: item.color }} />
              <span className="text-[10px] font-medium" style={{ color: colors.textSecondary }}>{item.label}</span>
            </div>
          ))}
        </div>

        {/* Hover tooltip (2D only — 3D uses three.js tooltips) */}
        {hoveredNode && !selectedNode && viewMode === '2d' && (
          <div
            className="fixed px-3 py-2 rounded-lg border pointer-events-none z-50"
            style={{
              left: tooltipPos.x,
              top: tooltipPos.y,
              backgroundColor: `${colors.bgSecondary}f0`,
              borderColor: colors.border,
              backdropFilter: 'blur(8px)',
              maxWidth: '14rem',
            }}
          >
            <div className="text-xs font-semibold truncate" style={{ color: colors.text }}>
              {hoveredNode.label as string}
            </div>
            <div className="text-[10px] font-medium mt-0.5" style={{ color: hoveredNode.color as string }}>
              {(hoveredNode.nodeType as string)?.charAt(0).toUpperCase() + (hoveredNode.nodeType as string)?.slice(1)}
            </div>
            {(hoveredNode.metadata as any)?.filePath && (
              <div className="text-[9px] mt-1 truncate font-mono" style={{ color: colors.textMuted }}>
                {(hoveredNode.metadata as any).filePath}
              </div>
            )}
          </div>
        )}

        {/* Selected node panel */}
        {selectedNode && (
          <div
            className="absolute top-3 right-3 w-64 rounded-lg border overflow-hidden"
            style={{
              backgroundColor: `${colors.bgSecondary}f5`,
              borderColor: colors.border,
              backdropFilter: 'blur(12px)',
            }}
          >
            <div className="flex items-center justify-between px-3 py-2 border-b" style={{ borderColor: colors.border }}>
              <span className="text-xs font-semibold truncate" style={{ color: colors.text }}>
                {selectedNode.label as string}
              </span>
              <button
                onClick={() => setSelectedNode(null)}
                className="w-5 h-5 rounded flex items-center justify-center"
                style={{ color: colors.textTertiary }}
              >
                <X className="w-3 h-3" />
              </button>
            </div>
            <div className="px-3 py-2 space-y-2">
              <InfoRow label="Type" value={(selectedNode.nodeType as string)} color={selectedNode.color as string} />
              {(selectedNode.metadata as any)?.score != null && (
                <InfoRow label="Relevance" value={`${((selectedNode.metadata as any).score * 100).toFixed(0)}%`} />
              )}
              {(selectedNode.metadata as any)?.connections != null && (
                <InfoRow label="Connections" value={String((selectedNode.metadata as any).connections)} />
              )}
              {(selectedNode.metadata as any)?.documentCount != null && (
                <InfoRow label="Documents" value={String((selectedNode.metadata as any).documentCount)} />
              )}
              {(selectedNode.metadata as any)?.space && (
                <InfoRow label="Space" value={(selectedNode.metadata as any).space} />
              )}
              {(selectedNode.metadata as any)?.filePath && (
                <div className="pt-1">
                  <span className="text-[10px]" style={{ color: colors.textMuted }}>Path</span>
                  <p className="text-[10px] font-mono mt-0.5 break-all" style={{ color: colors.textSecondary }}>
                    {(selectedNode.metadata as any).filePath}
                  </p>
                </div>
              )}
            </div>
          </div>
        )}
      </div>
    </div>
  );
}

function InfoRow({ label, value, color }: { label: string; value: string; color?: string }) {
  const { colors } = useTheme();
  return (
    <div className="flex items-center justify-between">
      <span className="text-[10px]" style={{ color: colors.textMuted }}>{label}</span>
      <span className="text-[10px] font-semibold" style={{ color: color || colors.text }}>{value}</span>
    </div>
  );
}
