import React, { useState } from 'react';
import { motion, AnimatePresence } from 'framer-motion';
import { Wrench, ChevronDown, ChevronRight, Clock, CheckCircle2, XCircle, Loader2 } from 'lucide-react';

export interface ToolInvocation {
  tool_name: string;
  arguments: Record<string, any>;
  result: string;
  success: boolean;
  duration_ms: number;
  status: 'pending' | 'running' | 'completed' | 'failed';
}

interface ToolCallBubbleProps {
  invocations: ToolInvocation[];
  colors: Record<string, string>;
  theme: string;
}

export const ToolCallBubble: React.FC<ToolCallBubbleProps> = ({ invocations, colors, theme }) => {
  const [expanded, setExpanded] = useState(false);
  const [expandedIdx, setExpandedIdx] = useState<number | null>(null);

  if (!invocations || invocations.length === 0) return null;

  const allDone = invocations.every(inv => inv.status === 'completed' || inv.status === 'failed');
  const allSuccess = invocations.every(inv => inv.status === 'completed' && inv.success);
  const totalMs = invocations.reduce((sum, inv) => sum + (inv.duration_ms || 0), 0);
  const toolNames = invocations.map(inv => inv.tool_name);
  const uniqueTools = [...new Set(toolNames)];
  const hasFailure = invocations.some(inv => inv.status === 'failed' || !inv.success);

  const statusIcon = (inv: ToolInvocation) => {
    switch (inv.status) {
      case 'pending':
        return <Clock className="w-3 h-3" style={{ color: colors.textMuted }} />;
      case 'running':
        return (
          <motion.div
            animate={{ rotate: 360 }}
            transition={{ duration: 1, repeat: Infinity, ease: 'linear' }}
          >
            <Loader2 className="w-3 h-3" style={{ color: '#f59e0b' }} />
          </motion.div>
        );
      case 'completed':
        return <CheckCircle2 className="w-3 h-3" style={{ color: '#10b981' }} />;
      case 'failed':
        return <XCircle className="w-3 h-3" style={{ color: '#ef4444' }} />;
    }
  };

  const statusColor = (inv: ToolInvocation) => {
    switch (inv.status) {
      case 'pending': return colors.textMuted;
      case 'running': return '#f59e0b';
      case 'completed': return '#10b981';
      case 'failed': return '#ef4444';
    }
  };

  const isDark = theme === 'dark';
  const summaryAccent = allSuccess ? '#10b981' : hasFailure ? '#ef4444' : '#f59e0b';

  // Compact summary mode when all tools are done
  if (allDone && !expanded) {
    const timeStr = totalMs < 1000 ? `${totalMs}ms` : `${(totalMs / 1000).toFixed(1)}s`;
    return (
      <motion.div
        initial={{ opacity: 0, y: 6 }}
        animate={{ opacity: 1, y: 0 }}
        className="my-2"
      >
        <button
          onClick={() => setExpanded(true)}
          className="w-full flex items-center gap-2 px-3 py-1.5 rounded-lg text-left transition-colors"
          style={{
            background: isDark ? `${summaryAccent}08` : `${summaryAccent}06`,
            borderLeft: `2px solid ${summaryAccent}60`,
          }}
          onMouseEnter={e => (e.currentTarget.style.background = isDark ? `${summaryAccent}12` : `${summaryAccent}0a`)}
          onMouseLeave={e => (e.currentTarget.style.background = isDark ? `${summaryAccent}08` : `${summaryAccent}06`)}
        >
          {allSuccess ? (
            <CheckCircle2 className="w-3 h-3 flex-shrink-0" style={{ color: summaryAccent }} />
          ) : (
            <Wrench className="w-3 h-3 flex-shrink-0" style={{ color: summaryAccent }} />
          )}
          <span className="text-[11px] flex-1" style={{ color: colors.textSecondary }}>
            Used {invocations.length} tool{invocations.length !== 1 ? 's' : ''}
            <span style={{ color: colors.textMuted }}> ({uniqueTools.join(', ')})</span>
            <span className="mx-1" style={{ color: colors.textMuted }}>·</span>
            <span className="tabular-nums" style={{ color: colors.textMuted }}>{timeStr}</span>
          </span>
          <ChevronRight className="w-3 h-3 flex-shrink-0" style={{ color: colors.textMuted }} />
        </button>
      </motion.div>
    );
  }

  return (
    <div className="my-2 space-y-1">
      {/* Collapse button when expanded */}
      {allDone && expanded && (
        <button
          onClick={() => { setExpanded(false); setExpandedIdx(null); }}
          className="flex items-center gap-1 text-[10px] px-2 py-1 rounded transition-colors mb-1"
          style={{ color: colors.textMuted }}
          onMouseEnter={e => (e.currentTarget.style.backgroundColor = isDark ? 'rgba(255,255,255,0.05)' : 'rgba(0,0,0,0.03)')}
          onMouseLeave={e => (e.currentTarget.style.backgroundColor = 'transparent')}
        >
          <ChevronDown className="w-3 h-3" />
          <span>Collapse</span>
        </button>
      )}
      {invocations.map((inv, idx) => {
        const isExpanded = expandedIdx === idx;
        return (
          <motion.div
            key={idx}
            initial={{ opacity: 0, y: 8 }}
            animate={{ opacity: 1, y: 0 }}
            transition={{ delay: idx * 0.1 }}
            className="rounded-lg overflow-hidden"
            style={{
              background: isDark ? 'rgba(255,255,255,0.03)' : 'rgba(0,0,0,0.02)',
              border: `1px solid ${isDark ? 'rgba(255,255,255,0.06)' : 'rgba(0,0,0,0.06)'}`,
            }}
          >
            {/* Header row — always visible */}
            <button
              onClick={() => setExpandedIdx(isExpanded ? null : idx)}
              className="w-full flex items-center gap-2 px-3 py-2 text-left hover:opacity-80 transition-opacity"
            >
              <Wrench className="w-3.5 h-3.5 flex-shrink-0" style={{ color: statusColor(inv) }} />
              <span className="text-xs font-medium flex-1 truncate" style={{ color: colors.text }}>
                {inv.tool_name}
              </span>
              {statusIcon(inv)}
              {inv.status === 'completed' && (
                <span className="text-[10px] tabular-nums" style={{ color: colors.textMuted }}>
                  {inv.duration_ms}ms
                </span>
              )}
              {isExpanded ? (
                <ChevronDown className="w-3 h-3 flex-shrink-0" style={{ color: colors.textMuted }} />
              ) : (
                <ChevronRight className="w-3 h-3 flex-shrink-0" style={{ color: colors.textMuted }} />
              )}
            </button>

            {/* Expanded details */}
            <AnimatePresence>
              {isExpanded && (
                <motion.div
                  initial={{ height: 0, opacity: 0 }}
                  animate={{ height: 'auto', opacity: 1 }}
                  exit={{ height: 0, opacity: 0 }}
                  transition={{ duration: 0.2 }}
                  className="overflow-hidden"
                >
                  <div className="px-3 pb-3 space-y-2">
                    {/* Arguments */}
                    {inv.arguments && Object.keys(inv.arguments).length > 0 && (
                      <div>
                        <span className="text-[10px] font-semibold uppercase tracking-wider" style={{ color: colors.textMuted }}>
                          Arguments
                        </span>
                        <pre
                          className="mt-1 text-[11px] p-2 rounded-md overflow-x-auto"
                          style={{
                            background: isDark ? 'rgba(0,0,0,0.3)' : 'rgba(0,0,0,0.04)',
                            color: colors.text,
                            fontFamily: 'monospace',
                          }}
                        >
                          {JSON.stringify(inv.arguments, null, 2)}
                        </pre>
                      </div>
                    )}

                    {/* Result */}
                    {inv.result && (
                      <div>
                        <span className="text-[10px] font-semibold uppercase tracking-wider" style={{ color: colors.textMuted }}>
                          Result
                        </span>
                        <pre
                          className="mt-1 text-[11px] p-2 rounded-md overflow-x-auto max-h-40"
                          style={{
                            background: isDark ? 'rgba(0,0,0,0.3)' : 'rgba(0,0,0,0.04)',
                            color: inv.success ? colors.text : '#ef4444',
                            fontFamily: 'monospace',
                          }}
                        >
                          {inv.result.length > 500 ? inv.result.slice(0, 500) + '...' : inv.result}
                        </pre>
                      </div>
                    )}
                  </div>
                </motion.div>
              )}
            </AnimatePresence>
          </motion.div>
        );
      })}
    </div>
  );
};

export default ToolCallBubble;
