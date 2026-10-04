import React, { useCallback, useEffect, useId, useMemo, useState } from 'react';
import { forceCenter, forceCollide, forceLink, forceManyBody, forceSimulation } from 'd3-force';
import type { SimulationLinkDatum, SimulationNodeDatum } from 'd3-force';
import { AlertTriangle, Globe, List, Loader2, Network, RefreshCw } from 'lucide-react';
import { cn } from '../../lib/utils';
import { onResearchChanged, toResearchError } from './api';
import { graphApi, onGraphProgress } from './graphApi';
import { DEFAULT_FILTERS, graphList, nodeRadius, progressLabel, shapeGraph, shortLabel, yearBounds } from './graphModel';
import type { DrawnNode, GraphFilters } from './graphModel';
import type { BuildProgress, GraphStatus, GraphViewData } from './graphTypes';
import { BUTTON, FOCUS_RING, INPUT, PRIMARY_BUTTON, SECTION_TITLE } from './ui';

type Load<T> = { status: 'loading' } | { status: 'ready'; value: T } | { status: 'error'; message: string };

const WIDTH = 880;
const HEIGHT = 520;
/** Ticks of the force layout, computed up front: the drawing is static (no motion). */
const LAYOUT_TICKS = 300;

interface Placed extends SimulationNodeDatum {
  node: DrawnNode;
}

/** Positions for the drawn nodes (a static force layout). */
function layout(nodes: DrawnNode[], edges: { source: string; target: string }[]): Map<string, { x: number; y: number }> {
  const placed: Placed[] = nodes.map((node, i) => ({
    node,
    // Deterministic start on a spiral, so the same graph lays out the same way.
    x: WIDTH / 2 + Math.cos(i * 2.4) * (20 + i * 2),
    y: HEIGHT / 2 + Math.sin(i * 2.4) * (20 + i * 2),
  }));
  const index = new Map(placed.map(p => [p.node.id, p]));
  const links: SimulationLinkDatum<Placed>[] = edges
    .map(e => ({ source: index.get(e.source), target: index.get(e.target) }))
    .filter((l): l is { source: Placed; target: Placed } => Boolean(l.source && l.target));
  const simulation = forceSimulation(placed)
    .force('charge', forceManyBody<Placed>().strength(p => (p.node.kind === 'library' ? -260 : -60)))
    .force('link', forceLink<Placed, SimulationLinkDatum<Placed>>(links).distance(60).strength(0.4))
    .force('collide', forceCollide<Placed>(p => nodeRadius(p.node) + 3))
    .force('center', forceCenter(WIDTH / 2, HEIGHT / 2))
    .stop();
  for (let i = 0; i < LAYOUT_TICKS; i++) simulation.tick();
  const out = new Map<string, { x: number; y: number }>();
  for (const p of placed) {
    const r = nodeRadius(p.node);
    out.set(p.node.id, {
      x: Math.max(r + 4, Math.min(WIDTH - r - 4, p.x ?? WIDTH / 2)),
      y: Math.max(r + 4, Math.min(HEIGHT - r - 4, p.y ?? HEIGHT / 2)),
    });
  }
  return out;
}

const FILL: Record<DrawnNode['kind'], string> = {
  library: 'var(--c-accent)',
  external: 'var(--c-text-faint)',
  cluster: 'var(--c-raised-2)',
};

/**
 * Library → Graph: the citation graph of the user's papers as a force-directed
 * drawing (library papers large and labelled, cited works small and dimmed,
 * works beyond the drawing cap folded into clusters) and as a keyboard list of
 * the same papers. Filters: library only, year range, method. A paper opens
 * its page.
 */
export function PaperGraphView({ onOpenPaper }: { onOpenPaper: (id: string) => void }) {
  const ids = { library: useId(), from: useId(), to: useId(), method: useId(), online: useId(), title: useId() };
  const [status, setStatus] = useState<Load<GraphStatus>>({ status: 'loading' });
  const [data, setData] = useState<Load<GraphViewData>>({ status: 'loading' });
  const [filters, setFilters] = useState<GraphFilters>(DEFAULT_FILTERS);
  const [mode, setMode] = useState<'map' | 'list'>('map');
  const [online, setOnline] = useState(false);
  const [progress, setProgress] = useState<BuildProgress | null>(null);
  const [building, setBuilding] = useState(false);
  const [buildError, setBuildError] = useState<string | null>(null);
  const [hovered, setHovered] = useState<string | null>(null);
  const [tick, setTick] = useState(0);

  useEffect(() => {
    let cancelled = false;
    graphApi.status().then(
      value => !cancelled && setStatus({ status: 'ready', value }),
      error => !cancelled && setStatus({ status: 'error', message: toResearchError(error).message }),
    );
    graphApi.view().then(
      value => !cancelled && setData({ status: 'ready', value }),
      error => !cancelled && setData({ status: 'error', message: toResearchError(error).message }),
    );
    return () => {
      cancelled = true;
    };
  }, [tick]);

  useEffect(() => {
    let disposed = false;
    const unlisteners: (() => void)[] = [];
    const keep = (fn: () => void) => (disposed ? fn() : unlisteners.push(fn));
    onGraphProgress(p => setProgress(p)).then(keep).catch(() => undefined);
    onResearchChanged(change => {
      if (change.kind === 'graph') setTick(t => t + 1);
    })
      .then(keep)
      .catch(() => undefined);
    return () => {
      disposed = true;
      unlisteners.forEach(fn => fn());
    };
  }, []);

  const build = useCallback(async () => {
    setBuilding(true);
    setBuildError(null);
    setProgress(null);
    try {
      await graphApi.build(online);
      setTick(t => t + 1);
    } catch (error) {
      setBuildError(toResearchError(error).message);
    } finally {
      setBuilding(false);
      setProgress(null);
    }
  }, [online]);

  const view = data.status === 'ready' ? data.value : null;
  const shaped = useMemo(() => (view ? shapeGraph(view, filters) : null), [view, filters]);
  const positions = useMemo(() => (shaped ? layout(shaped.nodes, shaped.edges) : new Map()), [shaped]);
  const rows = useMemo(() => (view ? graphList(view, filters) : []), [view, filters]);
  const bounds = useMemo(() => (view ? yearBounds(view.nodes) : null), [view]);

  const report = status.status === 'ready' ? status.value.report : null;
  const st = status.status === 'ready' ? status.value : null;
  const empty = view !== null && view.nodes.length === 0;
  const hoveredNode = shaped?.nodes.find(n => n.id === hovered) ?? null;

  const open = (node: DrawnNode) => {
    if (node.kind === 'cluster' && node.parent) onOpenPaper(node.parent);
    else if (node.kind !== 'cluster') onOpenPaper(node.id);
  };

  return (
    <div className="flex flex-col gap-4">
      <div className="flex flex-wrap items-center gap-3">
        <button type="button" onClick={() => void build()} disabled={building || st?.building} className={PRIMARY_BUTTON}>
          {building ? <Loader2 className="w-4 h-4 animate-spin motion-reduce:animate-none" aria-hidden="true" /> : <RefreshCw className="w-4 h-4" aria-hidden="true" />}
          {report ? 'Rebuild graph' : 'Build graph'}
        </button>
        <label htmlFor={ids.online} className={cn('inline-flex items-center gap-1.5 text-[12.5px]', st?.onlineAllowed ? 'text-shodh-text-secondary' : 'text-shodh-text-faint')}>
          <input
            id={ids.online}
            type="checkbox"
            checked={online && Boolean(st?.onlineAllowed)}
            disabled={!st?.onlineAllowed || building}
            onChange={e => setOnline(e.target.checked)}
            className={cn('w-3.5 h-3.5 accent-[var(--c-accent)]', FOCUS_RING)}
            aria-describedby={`${ids.online}-help`}
          />
          <Globe className="w-3.5 h-3.5" aria-hidden="true" />
          Look papers up on OpenAlex
        </label>
        <p id={`${ids.online}-help`} className="text-[11.5px] text-shodh-text-muted basis-full">
          {st && !st.onlineAllowed
            ? `${st.onlineBlockedReason ?? 'Web access is off.'} The graph is built from your PDFs only.`
            : 'Only reference identifiers (DOI, arXiv id) and reference titles are sent — never your documents’ text or your own papers’ titles. Answers are cached on this computer.'}
        </p>
      </div>

      {(building || progress) && (
        <p role="status" className="flex items-center gap-2 text-[12.5px] text-shodh-text-muted">
          <Loader2 className="w-4 h-4 animate-spin motion-reduce:animate-none" aria-hidden="true" />
          {progress ? progressLabel(progress) : 'Starting…'}
        </p>
      )}
      {buildError && (
        <p role="alert" className="flex items-start gap-2 text-[12.5px] text-shodh-text-secondary">
          <AlertTriangle className="w-4 h-4 mt-0.5 shrink-0 text-shodh-warning" aria-hidden="true" />
          {`The graph could not be built: ${buildError}`}
        </p>
      )}
      {report && (
        <p className="text-[12px] text-shodh-text-muted">
          {`${report.size.libraryPapers} library papers, ${report.size.papers - report.size.libraryPapers} cited works, ${report.size.cites} citations · ${report.identifiableReferences} of ${report.references} references read`}
          {report.online || report.referencesResolved > 0 ? ` · ${report.referencesResolved} matched on OpenAlex` : ''}
          {report.halted ? ` · lookups stopped: ${report.halted}` : ''}
          {report.failed.length > 0 ? ` · ${report.failed.length} files could not be read` : ''}
        </p>
      )}

      {data.status === 'loading' || status.status === 'loading' ? (
        <p role="status" className="flex items-center gap-2 text-[12.5px] text-shodh-text-muted">
          <Loader2 className="w-4 h-4 animate-spin motion-reduce:animate-none" aria-hidden="true" />
          Loading the graph…
        </p>
      ) : data.status === 'error' ? (
        <p role="alert" className="flex items-start gap-2 text-[12.5px] text-shodh-text-secondary">
          <AlertTriangle className="w-4 h-4 mt-0.5 shrink-0 text-shodh-warning" aria-hidden="true" />
          {`The graph could not be loaded: ${data.message}`}
        </p>
      ) : empty ? (
        <p className="text-[12.5px] text-shodh-text-muted">
          No graph yet. Build it to see which of your papers cite each other and what they build on.
        </p>
      ) : view && shaped ? (
        <>
          <div className="flex flex-wrap items-end gap-3">
            <label htmlFor={ids.library} className="inline-flex items-center gap-1.5 text-[12.5px] text-shodh-text-secondary h-8">
              <input id={ids.library} type="checkbox" checked={filters.libraryOnly} onChange={e => setFilters(f => ({ ...f, libraryOnly: e.target.checked }))} className={cn('w-3.5 h-3.5 accent-[var(--c-accent)]', FOCUS_RING)} />
              Library papers only
            </label>
            <div className="flex flex-col gap-1 w-24">
              <label htmlFor={ids.from} className="text-[12px] font-medium text-shodh-text-secondary">From year</label>
              <input id={ids.from} type="number" inputMode="numeric" className={INPUT} placeholder={bounds ? String(bounds.min) : ''} value={filters.yearFrom ?? ''} onChange={e => setFilters(f => ({ ...f, yearFrom: e.target.value ? Number(e.target.value) : null }))} />
            </div>
            <div className="flex flex-col gap-1 w-24">
              <label htmlFor={ids.to} className="text-[12px] font-medium text-shodh-text-secondary">To year</label>
              <input id={ids.to} type="number" inputMode="numeric" className={INPUT} placeholder={bounds ? String(bounds.max) : ''} value={filters.yearTo ?? ''} onChange={e => setFilters(f => ({ ...f, yearTo: e.target.value ? Number(e.target.value) : null }))} />
            </div>
            {view.methods.length > 0 && (
              <div className="flex flex-col gap-1 min-w-[180px]">
                <label htmlFor={ids.method} className="text-[12px] font-medium text-shodh-text-secondary">Method</label>
                <select id={ids.method} className={INPUT} value={filters.method ?? ''} onChange={e => setFilters(f => ({ ...f, method: e.target.value || null }))}>
                  <option value="">Every method</option>
                  {view.methods.map(m => (
                    <option key={m.id} value={m.id}>{m.label}</option>
                  ))}
                </select>
              </div>
            )}
            <div role="group" aria-label="Show as" className="ml-auto inline-flex gap-1">
              <button type="button" aria-pressed={mode === 'map'} onClick={() => setMode('map')} className={cn(BUTTON, mode === 'map' && 'bg-shodh-raised text-shodh-text')}>
                <Network className="w-3.5 h-3.5" aria-hidden="true" />
                Map
              </button>
              <button type="button" aria-pressed={mode === 'list'} onClick={() => setMode('list')} className={cn(BUTTON, mode === 'list' && 'bg-shodh-raised text-shodh-text')}>
                <List className="w-3.5 h-3.5" aria-hidden="true" />
                List
              </button>
            </div>
          </div>

          {mode === 'map' ? (
            <figure className="flex flex-col gap-2 m-0">
              <div className="relative rounded-xl border border-shodh-border bg-shodh-surface overflow-hidden">
                <svg viewBox={`0 0 ${WIDTH} ${HEIGHT}`} className="w-full h-auto block" role="group" aria-labelledby={ids.title}>
                  <title id={ids.title}>{`Citation graph: ${shaped.nodes.length} nodes drawn of ${shaped.matching} matching papers. Use the List view for a keyboard-friendly version.`}</title>
                  <g stroke="var(--c-border-strong)" strokeWidth={1} strokeOpacity={0.6}>
                    {shaped.edges.map(e => {
                      const a = positions.get(e.source);
                      const b = positions.get(e.target);
                      if (!a || !b) return null;
                      const active = hovered !== null && (e.source === hovered || e.target === hovered);
                      return <line key={`${e.source}>${e.target}`} x1={a.x} y1={a.y} x2={b.x} y2={b.y} stroke={active ? 'var(--c-accent)' : undefined} strokeWidth={active ? 2 : 1} />;
                    })}
                  </g>
                  {shaped.nodes.map(n => {
                    const p = positions.get(n.id);
                    if (!p) return null;
                    const r = nodeRadius(n);
                    return (
                      <g
                        key={n.id}
                        transform={`translate(${p.x},${p.y})`}
                        role="button"
                        tabIndex={n.kind === 'library' ? 0 : -1}
                        aria-label={`${n.label}${n.year ? `, ${n.year}` : ''}${n.kind === 'library' ? ', in your library' : ''}`}
                        className={cn('cursor-pointer outline-none [&:focus-visible>circle]:stroke-[var(--c-text)]')}
                        onClick={() => open(n)}
                        onKeyDown={e => {
                          if (e.key === 'Enter' || e.key === ' ') {
                            e.preventDefault();
                            open(n);
                          }
                        }}
                        onMouseEnter={() => setHovered(n.id)}
                        onMouseLeave={() => setHovered(h => (h === n.id ? null : h))}
                        onFocus={() => setHovered(n.id)}
                        onBlur={() => setHovered(h => (h === n.id ? null : h))}
                      >
                        <circle r={r + 6} fill="transparent" />
                        <circle r={r} fill={FILL[n.kind]} fillOpacity={n.kind === 'external' ? 0.55 : 1} stroke="var(--c-surface)" strokeWidth={2} />
                        {n.kind !== 'external' && (
                          <text y={r + 12} textAnchor="middle" className="fill-[var(--c-text-secondary)] text-[10px] pointer-events-none select-none">
                            {shortLabel(n.label, 28)}
                          </text>
                        )}
                      </g>
                    );
                  })}
                </svg>
                {hoveredNode && (
                  <div role="tooltip" className="absolute left-3 top-3 max-w-[60%] rounded-lg border border-shodh-border bg-shodh-raised px-3 py-2 text-[12px] text-shodh-text shadow-sm pointer-events-none">
                    <p className="font-medium">{shortLabel(hoveredNode.label, 120)}</p>
                    <p className="text-shodh-text-muted">
                      {hoveredNode.kind === 'cluster'
                        ? 'Works cited only here that are not drawn; open the paper to see them.'
                        : `${hoveredNode.year ?? 'Year unknown'} · ${hoveredNode.kind === 'library' ? 'in your library' : 'cited work'} · cited by ${hoveredNode.weight} library ${hoveredNode.weight === 1 ? 'paper' : 'papers'}`}
                    </p>
                  </div>
                )}
              </div>
              <figcaption className="flex flex-wrap items-center gap-4 text-[11.5px] text-shodh-text-muted">
                <span className="inline-flex items-center gap-1.5"><svg width="12" height="12" aria-hidden="true"><circle cx="6" cy="6" r="5" fill="var(--c-accent)" /></svg>In your library (labelled)</span>
                <span className="inline-flex items-center gap-1.5"><svg width="12" height="12" aria-hidden="true"><circle cx="6" cy="6" r="3" fill="var(--c-text-faint)" fillOpacity={0.55} /></svg>Cited work</span>
                <span className="inline-flex items-center gap-1.5"><svg width="12" height="12" aria-hidden="true"><circle cx="6" cy="6" r="5" fill="var(--c-raised-2)" stroke="var(--c-border-strong)" /></svg>More references (not drawn)</span>
                {shaped.clustered > 0 && <span>{`${shaped.clustered} works folded into clusters to keep the drawing readable.`}</span>}
                {view.omitted > 0 && <span>{`${view.omitted} least-cited works are not shown at all.`}</span>}
              </figcaption>
            </figure>
          ) : (
            <section aria-label="Papers in the graph" className="flex flex-col gap-2">
              <h3 className={SECTION_TITLE}>{`${rows.length} papers`}</h3>
              <ul className="flex flex-col divide-y divide-shodh-border-subtle rounded-xl border border-shodh-border max-h-[520px] overflow-y-auto scrollbar-thin">
                {rows.map(row => (
                  <li key={row.node.id}>
                    <button type="button" onClick={() => onOpenPaper(row.node.id)} className={cn('w-full text-left px-3 py-2 flex items-start gap-3 hover:bg-shodh-raised', FOCUS_RING)}>
                      <span className={cn('mt-1 w-2 h-2 rounded-full shrink-0', row.node.inLibrary ? 'bg-shodh-accent' : 'bg-shodh-text-faint')} aria-hidden="true" />
                      <span className="flex-1 min-w-0">
                        <span className={cn('block text-[13px] truncate', row.node.inLibrary ? 'text-shodh-text font-medium' : 'text-shodh-text-secondary')}>{row.node.label}</span>
                        <span className="block text-[11.5px] text-shodh-text-muted">
                          {[row.node.year ?? 'Year unknown', row.node.inLibrary ? 'in your library' : 'cited work', `cites ${row.cites}`, `cited by ${row.citedBy}`].join(' · ')}
                        </span>
                      </span>
                    </button>
                  </li>
                ))}
              </ul>
            </section>
          )}
        </>
      ) : null}
    </div>
  );
}
