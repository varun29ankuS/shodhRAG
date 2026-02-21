import React, { useState, useEffect, useRef, useMemo } from 'react';
import { invoke } from '@tauri-apps/api/core';
import {
  FileText,
  Search,
  Clock,
  AlertCircle,
  RefreshCw,
  HardDrive,
  Layers,
  AlertTriangle,
} from 'lucide-react';
import {
  LineChart,
  Line,
  XAxis,
  YAxis,
  CartesianGrid,
  Tooltip,
  ResponsiveContainer,
  Area,
  AreaChart,
} from 'recharts';
import { useTheme } from '../contexts/ThemeContext';

interface DashboardData {
  overview: {
    total_documents: number;
    total_chunks: number;
    total_queries: number;
    total_errors: number;
    avg_response_time_ms: number;
    storage_used_mb: number;
    uptime_seconds: number;
    error_rate_percent: number;
    queries_last_hour: number;
  };
  performance_chart: { timestamp: string; value: number; label?: string }[];
  usage_chart: { timestamp: string; value: number; label?: string }[];
  top_queries: {
    query: string;
    count: number;
    avg_time_ms: number;
    avg_results: number;
    last_used: string;
  }[];
}

function formatUptime(seconds: number): string {
  if (seconds < 60) return `${Math.floor(seconds)}s`;
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m`;
  const h = Math.floor(seconds / 3600);
  const m = Math.floor((seconds % 3600) / 60);
  return m > 0 ? `${h}h ${m}m` : `${h}h`;
}

function formatStorage(mb: number): string {
  if (mb < 1) return `${(mb * 1024).toFixed(0)} KB`;
  if (mb < 1024) return `${mb.toFixed(1)} MB`;
  return `${(mb / 1024).toFixed(2)} GB`;
}

function StatCard({
  icon: Icon,
  label,
  value,
  subtitle,
  color,
}: {
  icon: React.ElementType;
  label: string;
  value: string;
  subtitle?: string;
  color: string;
}) {
  const { colors } = useTheme();
  return (
    <div
      className="p-4 rounded-lg border"
      style={{ borderColor: colors.border, backgroundColor: colors.cardBg }}
    >
      <div className="flex items-center gap-2 mb-2">
        <div
          className="w-7 h-7 rounded-md flex items-center justify-center"
          style={{ backgroundColor: `${color}18` }}
        >
          <Icon className="w-3.5 h-3.5" style={{ color }} />
        </div>
        <span className="text-[10px] font-semibold tracking-wide" style={{ color: colors.textMuted }}>
          {label.toUpperCase()}
        </span>
      </div>
      <div className="text-xl font-bold" style={{ color: colors.text }}>{value}</div>
      {subtitle && (
        <div className="text-[10px] mt-1" style={{ color: colors.textMuted }}>{subtitle}</div>
      )}
    </div>
  );
}

export default function AnalyticsDashboard() {
  const { colors } = useTheme();
  const [data, setData] = useState<DashboardData | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const pollRef = useRef<ReturnType<typeof setInterval> | null>(null);

  const fetchData = async () => {
    try {
      const result = await invoke<DashboardData>('get_dashboard_data');
      setData(result);
      setError(null);
    } catch (err) {
      setError(`Failed to load analytics: ${err}`);
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    fetchData();
    pollRef.current = setInterval(fetchData, 30000);
    return () => {
      if (pollRef.current) clearInterval(pollRef.current);
    };
  }, []);

  const usageData = useMemo(() =>
    data ? data.usage_chart.map((p) => ({
      name: p.label || '',
      queries: p.value,
    })) : [],
    [data]
  );

  const perfData = useMemo(() =>
    data ? data.performance_chart.map((p) => ({
      name: p.label || '',
      ms: Math.round(p.value),
    })) : [],
    [data]
  );

  if (loading) {
    return (
      <div className="h-full flex items-center justify-center">
        <div className="text-center space-y-3">
          <div
            className="w-8 h-8 border-2 border-t-transparent rounded-full animate-spin mx-auto"
            style={{ borderColor: colors.primary }}
          />
          <p className="text-sm" style={{ color: colors.textMuted }}>Loading analytics...</p>
        </div>
      </div>
    );
  }

  if (error || !data) {
    return (
      <div className="h-full flex items-center justify-center">
        <div className="text-center space-y-3">
          <AlertCircle className="w-8 h-8 mx-auto" style={{ color: colors.error }} />
          <p className="text-sm" style={{ color: colors.textSecondary }}>{error || 'No data available'}</p>
          <button
            onClick={fetchData}
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

  const chartStroke = colors.primary;
  const chartGrid = colors.border;
  const chartText = colors.textMuted;

  const hasQueries = data.overview.total_queries > 0;

  return (
    <div className="h-full overflow-y-auto p-6">
      <div className="max-w-6xl mx-auto space-y-6">
        {/* Header */}
        <div className="flex items-center justify-between">
          <div>
            <h1 className="text-lg font-bold" style={{ color: colors.text }}>Analytics</h1>
            <p className="text-xs mt-0.5" style={{ color: colors.textMuted }}>
              Usage data from this session and previous sessions
            </p>
          </div>
          <button
            onClick={fetchData}
            className="w-7 h-7 rounded-md flex items-center justify-center border transition-colors"
            style={{ borderColor: colors.border, color: colors.textTertiary }}
            title="Refresh"
          >
            <RefreshCw className="w-3.5 h-3.5" />
          </button>
        </div>

        {/* Stat cards */}
        <div className="grid grid-cols-2 md:grid-cols-4 gap-4">
          <StatCard
            icon={FileText}
            label="Documents"
            value={data.overview.total_documents.toLocaleString()}
            subtitle={`${data.overview.total_chunks.toLocaleString()} chunks indexed`}
            color={colors.primary}
          />
          <StatCard
            icon={Search}
            label="Queries"
            value={data.overview.total_queries.toLocaleString()}
            subtitle={`${data.overview.queries_last_hour} this hour`}
            color={colors.secondary}
          />
          <StatCard
            icon={Clock}
            label="Avg Response"
            value={hasQueries ? `${data.overview.avg_response_time_ms.toFixed(0)}ms` : '—'}
            subtitle={hasQueries && data.overview.total_errors > 0
              ? `${data.overview.error_rate_percent.toFixed(1)}% error rate (${data.overview.total_errors} errors)`
              : hasQueries ? 'No errors' : 'No queries yet'}
            color={colors.success}
          />
          <StatCard
            icon={HardDrive}
            label="Storage"
            value={formatStorage(data.overview.storage_used_mb)}
            subtitle={`Uptime: ${formatUptime(data.overview.uptime_seconds)}`}
            color={colors.accent}
          />
        </div>

        {/* Charts row */}
        {hasQueries ? (
          <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
            {/* Usage chart */}
            <div
              className="p-4 rounded-lg border"
              style={{ borderColor: colors.border, backgroundColor: colors.cardBg }}
            >
              <h3 className="text-xs font-semibold mb-4" style={{ color: colors.text }}>
                Queries (Last 24h)
              </h3>
              <ResponsiveContainer width="100%" height={180}>
                <AreaChart data={usageData}>
                  <defs>
                    <linearGradient id="queryGrad" x1="0" y1="0" x2="0" y2="1">
                      <stop offset="0%" stopColor={chartStroke} stopOpacity={0.2} />
                      <stop offset="100%" stopColor={chartStroke} stopOpacity={0} />
                    </linearGradient>
                  </defs>
                  <CartesianGrid strokeDasharray="3 3" stroke={chartGrid} />
                  <XAxis dataKey="name" tick={{ fill: chartText, fontSize: 10 }} axisLine={false} tickLine={false} />
                  <YAxis tick={{ fill: chartText, fontSize: 10 }} axisLine={false} tickLine={false} allowDecimals={false} />
                  <Tooltip
                    contentStyle={{
                      backgroundColor: colors.bgSecondary,
                      borderColor: colors.border,
                      borderRadius: 8,
                      fontSize: 11,
                      color: colors.text,
                    }}
                  />
                  <Area type="monotone" dataKey="queries" stroke={chartStroke} fill="url(#queryGrad)" strokeWidth={2} />
                </AreaChart>
              </ResponsiveContainer>
            </div>

            {/* Performance chart */}
            <div
              className="p-4 rounded-lg border"
              style={{ borderColor: colors.border, backgroundColor: colors.cardBg }}
            >
              <h3 className="text-xs font-semibold mb-4" style={{ color: colors.text }}>
                Response Time (Last 24h)
              </h3>
              <ResponsiveContainer width="100%" height={180}>
                <LineChart data={perfData}>
                  <CartesianGrid strokeDasharray="3 3" stroke={chartGrid} />
                  <XAxis dataKey="name" tick={{ fill: chartText, fontSize: 10 }} axisLine={false} tickLine={false} />
                  <YAxis tick={{ fill: chartText, fontSize: 10 }} axisLine={false} tickLine={false} unit="ms" />
                  <Tooltip
                    contentStyle={{
                      backgroundColor: colors.bgSecondary,
                      borderColor: colors.border,
                      borderRadius: 8,
                      fontSize: 11,
                      color: colors.text,
                    }}
                    formatter={(value: number) => [`${value}ms`, 'Response time']}
                  />
                  <Line type="monotone" dataKey="ms" stroke={colors.success} strokeWidth={2} dot={false} />
                </LineChart>
              </ResponsiveContainer>
            </div>
          </div>
        ) : (
          <div
            className="p-8 rounded-lg border text-center"
            style={{ borderColor: colors.border, backgroundColor: colors.cardBg }}
          >
            <Search className="w-8 h-8 mx-auto mb-3" style={{ color: colors.textMuted }} />
            <p className="text-sm font-medium" style={{ color: colors.textSecondary }}>No query data yet</p>
            <p className="text-xs mt-1" style={{ color: colors.textMuted }}>
              Charts will appear after you start searching your documents
            </p>
          </div>
        )}

        {/* Top queries */}
        <div
          className="p-4 rounded-lg border"
          style={{ borderColor: colors.border, backgroundColor: colors.cardBg }}
        >
          <h3 className="text-xs font-semibold mb-3" style={{ color: colors.text }}>
            Top Queries
          </h3>
          <div className="space-y-1">
            {data.top_queries.length === 0 ? (
              <p className="text-xs py-4 text-center" style={{ color: colors.textMuted }}>No queries yet</p>
            ) : (
              <>
                {/* Header row */}
                <div className="flex items-center gap-3 px-2 py-1">
                  <span className="text-[10px] font-bold w-5" style={{ color: colors.textMuted }}>#</span>
                  <span className="text-[10px] font-bold flex-1" style={{ color: colors.textMuted }}>Query</span>
                  <span className="text-[10px] font-bold w-12 text-right" style={{ color: colors.textMuted }}>Count</span>
                  <span className="text-[10px] font-bold w-14 text-right" style={{ color: colors.textMuted }}>Avg Time</span>
                  <span className="text-[10px] font-bold w-16 text-right" style={{ color: colors.textMuted }}>Avg Results</span>
                </div>
                {data.top_queries.map((q, i) => (
                  <div
                    key={i}
                    className="flex items-center gap-3 px-2 py-1.5 rounded-md"
                    style={{ backgroundColor: i % 2 === 0 ? 'transparent' : colors.bgTertiary }}
                  >
                    <span className="text-[10px] font-bold w-5 text-center" style={{ color: colors.textMuted }}>
                      {i + 1}
                    </span>
                    <span className="text-xs flex-1 truncate" style={{ color: colors.textSecondary }}>
                      {q.query}
                    </span>
                    <span className="text-[10px] font-medium w-12 text-right" style={{ color: colors.text }}>
                      {q.count}
                    </span>
                    <span className="text-[10px] w-14 text-right" style={{ color: colors.textMuted }}>
                      {q.avg_time_ms.toFixed(0)}ms
                    </span>
                    <span className="text-[10px] w-16 text-right" style={{ color: colors.textMuted }}>
                      {q.avg_results.toFixed(1)}
                    </span>
                  </div>
                ))}
              </>
            )}
          </div>
        </div>

        {/* Error banner if errors exist */}
        {data.overview.total_errors > 0 && (
          <div
            className="p-3 rounded-lg border flex items-center gap-3"
            style={{ borderColor: `${colors.warning}40`, backgroundColor: `${colors.warning}0a` }}
          >
            <AlertTriangle className="w-4 h-4 shrink-0" style={{ color: colors.warning }} />
            <div>
              <p className="text-xs font-medium" style={{ color: colors.warning }}>
                {data.overview.total_errors} failed {data.overview.total_errors === 1 ? 'query' : 'queries'} ({data.overview.error_rate_percent.toFixed(1)}% error rate)
              </p>
              <p className="text-[10px] mt-0.5" style={{ color: colors.textMuted }}>
                Check your LLM configuration and document indexing if errors persist
              </p>
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
