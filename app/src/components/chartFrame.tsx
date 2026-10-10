/**
 * The parts of a chart drawn without recharts: its title and the raw
 * content shown when it cannot be parsed. Shared by the chart and its
 * loading placeholder, so the placeholder has the chart's exact frame.
 */

export const CHART_FONT = '"Geist Variable", system-ui, -apple-system, "Segoe UI", sans-serif';

export function chartTitleColor(theme: string): string {
  return theme === 'dark' ? '#e5e7eb' : '#1f2937';
}

export function ChartTitle({ title, color }: { title: string; color: string }) {
  return (
    <div style={{ padding: '12px 16px 4px' }}>
      <h3 style={{
        color,
        fontSize: 13,
        fontWeight: 600,
        fontFamily: CHART_FONT,
        margin: 0,
        letterSpacing: '-0.01em',
      }}>
        {title}
      </h3>
    </div>
  );
}

export function ChartUnparsed({ content }: { content: string }) {
  return (
    <div className="p-4">
      <pre className="text-xs font-mono whitespace-pre-wrap opacity-60">
        {content}
      </pre>
    </div>
  );
}
