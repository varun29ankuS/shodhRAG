/**
 * Cross-paper comparison helpers: a `Comparison` as table rows and as a
 * chart spec, paper results grouped for display, facet ordering and the
 * labels of values. Pure module, unit-tested with Node
 * (`app/tests/researchComparison.test.ts`).
 */

import type {
  Comparison,
  ComparisonCell,
  ComparisonColumn,
  FacetValue,
  ResultFacets,
  ResultRecord,
  ResultRegion,
} from './types.ts';

/** A value as printed, with its unit when the unit is not already in the text. */
export function valueLabel(cell: Pick<ComparisonCell, 'valueText' | 'value' | 'unit'>): string {
  const text = cell.valueText.trim() || cell.value;
  const unit = cell.unit?.trim();
  if (!unit) return text;
  return text.toLowerCase().includes(unit.toLowerCase()) ? text : `${text} ${unit}`;
}

/** "dataset · metric" (either part may be missing). */
export function columnLabel(column: Pick<ComparisonColumn, 'dataset' | 'metric'>): string {
  return [column.dataset.trim(), column.metric.trim()].filter(Boolean).join(' · ') || 'Value';
}

/** One table cell: the values reported for one method and column. */
export interface TableCell {
  key: string;
  values: ComparisonCell[];
}

export interface TableRow {
  method: string;
  methodId: string;
  cells: TableCell[];
}

/** The comparison as rows in column order (every row has a cell per column). */
export function comparisonRows(comparison: Comparison): TableRow[] {
  return comparison.rows.map(row => ({
    method: row.method,
    methodId: row.methodId,
    cells: comparison.columns.map(column => ({ key: column.key, values: row.cells[column.key] ?? [] })),
  }));
}

/** A number for charting, or null when the value is not a plain number. */
export function chartNumber(value: string): number | null {
  const cleaned = value.trim().replace(/[,\s]/g, '').replace(/[−–]/g, '-');
  if (!/^[+-]?(?:\d+(?:\.\d*)?|\.\d+)(?:[eE][+-]?\d+)?$/.test(cleaned)) return null;
  const n = Number(cleaned);
  return Number.isFinite(n) ? n : null;
}

/** Stable series key of a paper (chart data keys must be plain identifiers). */
function seriesKey(index: number): string {
  return `p${index + 1}`;
}

/** The ```chart spec of one column: methods on the x axis, one bar series per paper. */
export interface ColumnChartSpec {
  type: 'bar';
  title: string;
  xKey: 'method';
  series: { key: string; label: string }[];
  data: Record<string, string | number | null>[];
}

/**
 * A bar chart of one column: one category per method that has a value, one
 * series per paper that reported any. When a paper reports several values
 * for a method (different settings) the first is charted; `multiple` says
 * how many cells were not charted for that reason. Null when nothing in the
 * column is a number.
 */
export function columnChart(comparison: Comparison, columnKey: string): { spec: ColumnChartSpec; skipped: number; multiple: number } | null {
  const column = comparison.columns.find(c => c.key === columnKey);
  if (!column) return null;
  const papers: string[] = [];
  const names = new Map<string, string>();
  for (const p of comparison.papers) names.set(p.filePath, p.fileName);
  let skipped = 0;
  let multiple = 0;
  const data: Record<string, string | number | null>[] = [];
  for (const row of comparison.rows) {
    const cells = row.cells[columnKey] ?? [];
    const point: Record<string, string | number | null> = { method: row.method };
    let any = false;
    const seen = new Set<string>();
    for (const cell of cells) {
      if (seen.has(cell.filePath)) {
        multiple += 1;
        continue;
      }
      seen.add(cell.filePath);
      const n = chartNumber(cell.value);
      if (n === null) {
        skipped += 1;
        continue;
      }
      if (!papers.includes(cell.filePath)) {
        papers.push(cell.filePath);
        if (!names.has(cell.filePath)) names.set(cell.filePath, cell.fileName);
      }
      point[seriesKey(papers.indexOf(cell.filePath))] = n;
      any = true;
    }
    if (any) data.push(point);
  }
  if (data.length === 0) return null;
  for (const point of data) for (let i = 0; i < papers.length; i++) if (!(seriesKey(i) in point)) point[seriesKey(i)] = null;
  return {
    spec: {
      type: 'bar',
      title: columnLabel(column),
      xKey: 'method',
      series: papers.map((path, i) => ({ key: seriesKey(i), label: names.get(path) ?? path })),
      data,
    },
    skipped,
    multiple,
  };
}

/** Facet values, most used first, then alphabetically. */
export function sortFacet(values: readonly FacetValue[]): FacetValue[] {
  return [...values].sort((a, b) => b.count - a.count || a.label.localeCompare(b.label) || a.id.localeCompare(b.id));
}

/** Facets with each list sorted for the pickers. */
export function sortedFacets(facets: ResultFacets): ResultFacets {
  return {
    methods: sortFacet(facets.methods),
    datasets: sortFacet(facets.datasets),
    metrics: sortFacet(facets.metrics),
    papers: [...facets.papers].sort((a, b) => b.results - a.results || a.fileName.localeCompare(b.fileName)),
  };
}

/** Results of a paper grouped by table caption (in page order), for display. */
export function groupByTable(results: readonly ResultRecord[]): { caption: string; page: number; results: ResultRecord[] }[] {
  const groups = new Map<string, { caption: string; page: number; results: ResultRecord[] }>();
  for (const r of results) {
    const caption = r.tableCaption?.trim() || `Table on page ${r.page}`;
    const key = `${r.page}\u0000${caption}`;
    const group = groups.get(key) ?? { caption, page: r.page, results: [] };
    group.results.push(r);
    groups.set(key, group);
  }
  return [...groups.values()]
    .sort((a, b) => a.page - b.page || a.caption.localeCompare(b.caption))
    .map(g => ({
      ...g,
      results: [...g.results].sort(
        (a, b) => (a.region && b.region ? b.region.y1 - a.region.y1 || a.region.x0 - b.region.x0 : 0) || a.method.localeCompare(b.method),
      ),
    }));
}

/** "rule · 92%" style provenance of a result. */
export function provenanceLabel(r: Pick<ResultRecord, 'extractor' | 'confidence'>): string {
  const by = r.extractor === 'user' ? 'confirmed by you' : r.extractor === 'llm' ? 'model-assisted' : 'rule';
  const pct = Math.round(Math.min(1, Math.max(0, r.confidence)) * 100);
  return `${by} · ${pct}%`;
}

/** The regions to outline for a cell (its own box), or null. */
export function cellRegions(region: ResultRegion | null): ResultRegion[] | null {
  return region ? [region] : null;
}
