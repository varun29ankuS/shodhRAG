import { useMemo } from 'react';
import {
  BarChart, Bar, LineChart, Line, PieChart, Pie, Cell,
  XAxis, YAxis, CartesianGrid, Tooltip, Legend, ResponsiveContainer,
  RadarChart, Radar, PolarGrid, PolarAngleAxis, PolarRadiusAxis,
  AreaChart, Area, ScatterChart, Scatter,
} from 'recharts';
import type { Artifact } from './EnhancedArtifactPanel';
import { tryParseChartSpec } from '../utils/artifactExtractor';

interface ChartArtifactProps {
  artifact: Artifact;
  theme: string;
}

// Softer, more modern palette
const DEFAULT_COLORS = [
  '#6366f1', '#06b6d4', '#f59e0b', '#10b981', '#ef4444',
  '#8b5cf6', '#ec4899', '#14b8a6', '#f97316', '#3b82f6',
  '#a855f7', '#84cc16', '#e879f9', '#22d3ee', '#fb923c',
];

const FONT = '-apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, "Inter", sans-serif';

export function ChartArtifact({ artifact, theme }: ChartArtifactProps) {
  const isDark = theme === 'dark';
  const textColor = isDark ? '#9ca3af' : '#6b7280';
  const titleColor = isDark ? '#e5e7eb' : '#1f2937';
  const gridColor = isDark ? 'rgba(55,65,81,0.5)' : 'rgba(229,231,235,0.8)';
  const tooltipBg = isDark ? '#1e293b' : '#ffffff';
  const tooltipBorder = isDark ? '#334155' : '#e2e8f0';
  const tooltipText = isDark ? '#e2e8f0' : '#1e293b';
  const bgColor = isDark ? 'transparent' : 'transparent';

  const spec = useMemo(() => tryParseChartSpec(artifact.content), [artifact.content]);

  const rechartsData = useMemo(() => {
    if (!spec) return [];
    return spec.data.labels.map((label, i) => {
      const point: Record<string, string | number> = { name: label };
      spec.data.datasets.forEach((ds) => {
        point[ds.label] = ds.data[i] ?? 0;
      });
      return point;
    });
  }, [spec]);

  const pieData = useMemo(() => {
    if (!spec || !rechartsData.length) return [];
    return rechartsData.map((d) => ({
      name: d.name as string,
      value: (d[spec.data.datasets[0]?.label] as number) || 0,
    }));
  }, [spec, rechartsData]);

  if (!spec || rechartsData.length === 0) {
    return (
      <div className="p-4">
        <pre className="text-xs font-mono whitespace-pre-wrap opacity-60">
          {artifact.content}
        </pre>
      </div>
    );
  }

  const chartType = spec.type;

  const getColor = (idx: number, ds: typeof spec.data.datasets[0]) => {
    if (typeof ds.backgroundColor === 'string') return ds.backgroundColor;
    if (ds.borderColor) return ds.borderColor;
    return DEFAULT_COLORS[idx % DEFAULT_COLORS.length];
  };

  const getPieColor = (idx: number) => {
    const ds = spec.data.datasets[0];
    if (Array.isArray(ds?.backgroundColor) && ds.backgroundColor[idx]) return ds.backgroundColor[idx];
    return DEFAULT_COLORS[idx % DEFAULT_COLORS.length];
  };

  const tooltipStyle: React.CSSProperties = {
    backgroundColor: tooltipBg,
    borderColor: tooltipBorder,
    color: tooltipText,
    borderRadius: 8,
    fontSize: 12,
    fontFamily: FONT,
    padding: '8px 12px',
    boxShadow: isDark
      ? '0 4px 12px rgba(0,0,0,0.4)'
      : '0 4px 12px rgba(0,0,0,0.08)',
    border: `1px solid ${tooltipBorder}`,
  };

  const axisTick = { fill: textColor, fontSize: 11, fontFamily: FONT };
  const axisTickSmall = { fill: textColor, fontSize: 10, fontFamily: FONT };
  const margin = { top: 12, right: 16, left: 4, bottom: 4 };

  const legendStyle: React.CSSProperties = {
    fontSize: 11,
    fontFamily: FONT,
    color: textColor,
    paddingTop: 4,
  };

  const pieLabelColor = isDark ? '#d1d5db' : '#374151';

  const renderPieLabel = ({ name, percent, cx, x, y }: any) => {
    const pct = (percent * 100).toFixed(0);
    const anchor = x > cx ? 'start' : 'end';
    return (
      <text
        x={x}
        y={y}
        fill={pieLabelColor}
        textAnchor={anchor}
        dominantBaseline="central"
        style={{ fontSize: 12, fontFamily: FONT, fontWeight: 500 }}
      >
        {name} ({pct}%)
      </text>
    );
  };

  const renderChart = () => {
    switch (chartType) {
      case 'line':
        return (
          <LineChart data={rechartsData} margin={margin}>
            <CartesianGrid strokeDasharray="3 3" stroke={gridColor} vertical={false} />
            <XAxis dataKey="name" tick={axisTick} axisLine={false} tickLine={false} />
            <YAxis tick={axisTick} axisLine={false} tickLine={false} width={32} />
            <Tooltip contentStyle={tooltipStyle} itemStyle={{ color: tooltipText, fontSize: 12 }} />
            <Legend wrapperStyle={legendStyle} iconSize={8} />
            {spec.data.datasets.map((ds, i) => (
              <Line key={ds.label} type="monotone" dataKey={ds.label} stroke={getColor(i, ds)} strokeWidth={2} dot={{ fill: getColor(i, ds), r: 3, strokeWidth: 0 }} activeDot={{ r: 5, strokeWidth: 0 }} />
            ))}
          </LineChart>
        );

      case 'area':
        return (
          <AreaChart data={rechartsData} margin={margin}>
            <defs>
              {spec.data.datasets.map((ds, i) => (
                <linearGradient key={ds.label} id={`grad-${i}`} x1="0" y1="0" x2="0" y2="1">
                  <stop offset="0%" stopColor={getColor(i, ds)} stopOpacity={0.25} />
                  <stop offset="100%" stopColor={getColor(i, ds)} stopOpacity={0} />
                </linearGradient>
              ))}
            </defs>
            <CartesianGrid strokeDasharray="3 3" stroke={gridColor} vertical={false} />
            <XAxis dataKey="name" tick={axisTick} axisLine={false} tickLine={false} />
            <YAxis tick={axisTick} axisLine={false} tickLine={false} width={32} />
            <Tooltip contentStyle={tooltipStyle} itemStyle={{ color: tooltipText, fontSize: 12 }} />
            <Legend wrapperStyle={legendStyle} iconSize={8} />
            {spec.data.datasets.map((ds, i) => (
              <Area key={ds.label} type="monotone" dataKey={ds.label} stroke={getColor(i, ds)} fill={`url(#grad-${i})`} strokeWidth={2} />
            ))}
          </AreaChart>
        );

      case 'pie':
      case 'doughnut':
      case 'donut':
      case 'polararea':
        return (
          <PieChart>
            <Tooltip
              contentStyle={tooltipStyle}
              itemStyle={{ color: tooltipText, fontSize: 12 }}
              formatter={(value: number, name: string) => [`${value}`, name]}
            />
            <Legend wrapperStyle={legendStyle} iconSize={8} iconType="circle" />
            <Pie
              data={pieData}
              cx="50%"
              cy="50%"
              innerRadius={chartType === 'doughnut' || chartType === 'donut' ? 50 : 0}
              outerRadius={100}
              dataKey="value"
              paddingAngle={2}
              label={renderPieLabel}
              labelLine={{ stroke: isDark ? '#6b7280' : '#9ca3af', strokeWidth: 1 }}
              strokeWidth={0}
            >
              {pieData.map((_, i) => (
                <Cell key={i} fill={getPieColor(i)} />
              ))}
            </Pie>
          </PieChart>
        );

      case 'radar':
        return (
          <RadarChart cx="50%" cy="50%" outerRadius={90} data={rechartsData}>
            <PolarGrid stroke={gridColor} />
            <PolarAngleAxis dataKey="name" tick={axisTickSmall} />
            <PolarRadiusAxis tick={axisTickSmall} />
            <Tooltip contentStyle={tooltipStyle} itemStyle={{ color: tooltipText, fontSize: 12 }} />
            <Legend wrapperStyle={legendStyle} iconSize={8} />
            {spec.data.datasets.map((ds, i) => (
              <Radar key={ds.label} name={ds.label} dataKey={ds.label} stroke={getColor(i, ds)} fill={getColor(i, ds)} fillOpacity={0.15} strokeWidth={1.5} />
            ))}
          </RadarChart>
        );

      case 'scatter':
      case 'bubble':
        return (
          <ScatterChart margin={margin}>
            <CartesianGrid strokeDasharray="3 3" stroke={gridColor} vertical={false} />
            <XAxis dataKey="name" tick={axisTick} axisLine={false} tickLine={false} />
            <YAxis tick={axisTick} axisLine={false} tickLine={false} width={32} />
            <Tooltip contentStyle={tooltipStyle} itemStyle={{ color: tooltipText, fontSize: 12 }} />
            <Legend wrapperStyle={legendStyle} iconSize={8} />
            {spec.data.datasets.map((ds, i) => (
              <Scatter key={ds.label} name={ds.label} data={rechartsData} fill={getColor(i, ds)} />
            ))}
          </ScatterChart>
        );

      case 'horizontalbar':
      case 'horizontal_bar':
        return (
          <BarChart data={rechartsData} layout="vertical" margin={{ ...margin, left: 8 }}>
            <CartesianGrid strokeDasharray="3 3" stroke={gridColor} horizontal={false} />
            <XAxis type="number" tick={axisTick} axisLine={false} tickLine={false} />
            <YAxis dataKey="name" type="category" tick={axisTick} axisLine={false} tickLine={false} width={80} />
            <Tooltip contentStyle={tooltipStyle} itemStyle={{ color: tooltipText, fontSize: 12 }} />
            <Legend wrapperStyle={legendStyle} iconSize={8} />
            {spec.data.datasets.map((ds, i) => (
              <Bar key={ds.label} dataKey={ds.label} fill={getColor(i, ds)} radius={[0, 4, 4, 0]} barSize={20} />
            ))}
          </BarChart>
        );

      default:
        return (
          <BarChart data={rechartsData} margin={margin}>
            <CartesianGrid strokeDasharray="3 3" stroke={gridColor} vertical={false} />
            <XAxis dataKey="name" tick={axisTick} axisLine={false} tickLine={false} />
            <YAxis tick={axisTick} axisLine={false} tickLine={false} width={32} />
            <Tooltip contentStyle={tooltipStyle} itemStyle={{ color: tooltipText, fontSize: 12 }} />
            <Legend wrapperStyle={legendStyle} iconSize={8} />
            {spec.data.datasets.map((ds, i) => (
              <Bar key={ds.label} dataKey={ds.label} fill={getColor(i, ds)} radius={[4, 4, 0, 0]} barSize={28} />
            ))}
          </BarChart>
        );
    }
  };

  return (
    <div style={{ backgroundColor: bgColor }}>
      {spec.title && (
        <div style={{ padding: '12px 16px 4px' }}>
          <h3 style={{
            color: titleColor,
            fontSize: 13,
            fontWeight: 600,
            fontFamily: FONT,
            margin: 0,
            letterSpacing: '-0.01em',
          }}>
            {spec.title}
          </h3>
        </div>
      )}
      <div style={{ width: '100%', height: 320, padding: '4px 8px 8px' }}>
        <ResponsiveContainer width="100%" height="100%">
          {renderChart()}
        </ResponsiveContainer>
      </div>
    </div>
  );
}
