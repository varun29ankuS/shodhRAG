/**
 * The parts of a chart drawn without recharts: its title and the raw
 * content shown when it cannot be parsed. Shared by the chart and its
 * loading placeholder, so the placeholder has the chart's exact frame.
 */

export const CHART_FONT = '"Geist Variable", system-ui, -apple-system, "Segoe UI", sans-serif';

/** The title's colour: the theme's text token (the title is an HTML heading). */
export function chartTitleColor(): string {
  return 'var(--c-text)';
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
      <pre className="text-caption font-mono whitespace-pre-wrap opacity-60">
        {content}
      </pre>
    </div>
  );
}
