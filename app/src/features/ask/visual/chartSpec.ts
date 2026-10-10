/**
 * Chart blocks written by the agent as ```chart fences.
 *
 * Preferred shape (what the agent is told to emit):
 *   { "type": "bar", "title": "Recall@10 on SIFT1M", "xKey": "method",
 *     "series": [{ "key": "recall", "label": "Recall@10" }],
 *     "data": [{ "method": "HNSW", "recall": 0.98 }, ...] }
 *
 * The legacy Chart.js-like shape ({ type, data: { labels, datasets } }) is
 * still accepted. Both normalise to labels + datasets for ChartArtifact.
 */

export const CHART_KINDS = ['bar', 'line', 'area', 'scatter', 'pie'] as const;
export type ChartKind = typeof CHART_KINDS[number];

export interface NormalizedChart {
  type: ChartKind;
  title?: string;
  data: { labels: string[]; datasets: { label: string; data: (number | null)[] }[] };
}

/** Upper bounds keep a malformed or hostile block from freezing the renderer. */
const MAX_POINTS = 500;
const MAX_SERIES = 12;

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function toNumber(value: unknown): number | null {
  if (typeof value === 'number' && Number.isFinite(value)) return value;
  if (typeof value === 'string' && value.trim() !== '') {
    const n = Number(value.replace(/[,_\s]/g, ''));
    return Number.isFinite(n) ? n : null;
  }
  return null;
}

function kindOf(value: unknown): ChartKind | null {
  const k = typeof value === 'string' ? value.toLowerCase().trim() : '';
  return (CHART_KINDS as readonly string[]).includes(k) ? (k as ChartKind) : null;
}

export type ChartParseResult = { ok: true; chart: NormalizedChart } | { ok: false; error: string };

export function parseChartBlock(source: string): ChartParseResult {
  let parsed: unknown;
  try {
    parsed = JSON.parse(source);
  } catch {
    return { ok: false, error: 'The chart data is not valid JSON.' };
  }
  if (!isRecord(parsed)) return { ok: false, error: 'The chart must be a JSON object.' };
  const type = kindOf(parsed.type);
  if (!type) return { ok: false, error: `Unsupported chart type; use one of ${CHART_KINDS.join(', ')}.` };
  const title = typeof parsed.title === 'string' && parsed.title.trim() ? parsed.title.trim() : undefined;

  // Preferred shape: rows + xKey + series.
  if (Array.isArray(parsed.data) && typeof parsed.xKey === 'string') {
    const xKey = parsed.xKey;
    const rows = parsed.data.filter(isRecord).slice(0, MAX_POINTS);
    const series = (Array.isArray(parsed.series) ? parsed.series : [])
      .filter(isRecord)
      .filter(s => typeof s.key === 'string')
      .slice(0, MAX_SERIES)
      .map(s => ({ key: s.key as string, label: typeof s.label === 'string' && s.label ? s.label : (s.key as string) }));
    if (rows.length === 0) return { ok: false, error: 'The chart has no data rows.' };
    if (series.length === 0) return { ok: false, error: 'The chart names no series to plot.' };
    return {
      ok: true,
      chart: {
        type,
        title,
        data: {
          labels: rows.map(r => String(r[xKey] ?? '')),
          datasets: series.map(s => ({ label: s.label, data: rows.map(r => toNumber(r[s.key])) })),
        },
      },
    };
  }

  // Legacy shape: data.labels + data.datasets.
  if (isRecord(parsed.data) && Array.isArray(parsed.data.labels)) {
    const labels = parsed.data.labels.slice(0, MAX_POINTS).map(l => String(l ?? ''));
    const datasets = (Array.isArray(parsed.data.datasets) ? parsed.data.datasets : [])
      .filter(isRecord)
      .slice(0, MAX_SERIES)
      .map((d, i) => ({
        label: typeof d.label === 'string' && d.label ? d.label : `Series ${i + 1}`,
        data: (Array.isArray(d.data) ? d.data : []).slice(0, MAX_POINTS).map(toNumber),
      }));
    if (labels.length === 0 || datasets.length === 0) return { ok: false, error: 'The chart has no data.' };
    return { ok: true, chart: { type, title, data: { labels, datasets } } };
  }

  return { ok: false, error: 'The chart needs "xKey", "series" and "data" rows.' };
}
