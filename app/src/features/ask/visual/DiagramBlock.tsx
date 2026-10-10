import { useCallback, useEffect, useId, useMemo, useRef, useState } from 'react';
import { FileCode2, Loader2, MessageSquareText, Quote } from 'lucide-react';
import { cn } from '../../../lib/utils';
import { FocusFrame, useOpenFocus } from '../../focus/FocusFrame';
import { selectionTarget, svgTarget } from '../../focus/targets';
import { publishTarget } from '../../agent/navigation';
import { agentApi, toAgentError } from '../../agent/useAgentSession';
import { useAnswerBlocks } from './answerContext';
import { useDiagramHost } from './diagramHost';
import { describeDiagram, describeNode, parseDiagram } from './diagramSpec';
import type { DiagramSpec } from './diagramSpec';
import { layoutDiagram } from './diagramLayout';
import { diagramSvg, escapeXml } from './diagramSvg';
import { BlockError } from './VisualBlocks';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-shodh-ground';

const ACTION =
  'inline-flex items-center gap-1.5 h-7 px-2.5 rounded-md border border-shodh-border bg-shodh-surface text-[12px] font-medium text-shodh-text-secondary hover:bg-shodh-raised hover:text-shodh-text transition-colors duration-micro disabled:opacity-50 disabled:cursor-not-allowed';

type Files =
  | { status: 'none' }
  | { status: 'checking' }
  | { status: 'ready'; found: Map<string, string> }
  | { status: 'missing'; paths: string[] }
  | { status: 'error'; message: string };

/**
 * A ```diagram block: a typed graph the model wrote, checked against the
 * answer (its sources, and in Code mode the code folder) and drawn by
 * Shodh's own layout. Parts are buttons: one with a file opens it, one with
 * a source opens the passage, and any can be asked about.
 */
export function DiagramBlock({ source }: { source: string }) {
  const { citations, openCitation } = useAnswerBlocks();
  const { codeWorkspaceId } = useDiagramHost();
  const parsed = useMemo(
    () => parseDiagram(source.trim(), { citations, codeMode: codeWorkspaceId !== null }),
    [source, citations, codeWorkspaceId],
  );
  const spec = 'spec' in parsed ? parsed.spec : null;
  const pathsKey = 'paths' in parsed ? parsed.paths.join('\n') : '';
  // Paths are checked before the first drawing, so a missing file never flashes a diagram.
  const [files, setFiles] = useState<Files>(() => (pathsKey && codeWorkspaceId ? { status: 'checking' } : { status: 'none' }));

  useEffect(() => {
    const paths = pathsKey ? pathsKey.split('\n') : [];
    if (paths.length === 0 || !codeWorkspaceId) {
      setFiles({ status: 'none' });
      return;
    }
    let active = true;
    setFiles({ status: 'checking' });
    agentApi
      .codePaths(codeWorkspaceId, paths)
      .then(resolved => {
        if (!active) return;
        const missing = paths.filter((_, i) => !resolved[i]);
        if (missing.length > 0) setFiles({ status: 'missing', paths: missing });
        else setFiles({ status: 'ready', found: new Map(paths.map((p, i) => [p, resolved[i] as string])) });
      })
      .catch(error => {
        if (active) setFiles({ status: 'error', message: toAgentError(error).message });
      });
    return () => { active = false; };
  }, [pathsKey, codeWorkspaceId]);

  const problem = 'errors' in parsed
    ? parsed.errors.join(' ')
    : files.status === 'missing'
      ? `It names files that are not in the code folder: ${files.paths.join(', ')}.`
      : files.status === 'error'
        ? `The files it names could not be checked: ${files.message}`
        : null;
  const getErrorTarget = useCallback(
    () => selectionTarget({ text: source, context: problem ?? '', origin: 'answer' }),
    [source, problem],
  );

  if (problem !== null) {
    return <BlockError title="Diagram not drawn" message={problem} source={source} ask={{ noun: 'diagram', getTarget: getErrorTarget }} />;
  }
  if (!spec) return null;
  if (files.status === 'checking') {
    return (
      <div className="my-4 flex items-center justify-center gap-2 h-24 rounded-xl border border-shodh-border bg-shodh-surface text-[12.5px] text-shodh-text-muted" aria-busy="true">
        <Loader2 className="w-4 h-4 animate-spin motion-reduce:animate-none" aria-hidden="true" />
        Checking the files it names…
      </div>
    );
  }
  return (
    <DiagramView
      spec={spec}
      files={files.status === 'ready' ? files.found : null}
      openCitation={openCitation}
    />
  );
}

interface DiagramViewProps {
  spec: DiagramSpec;
  /** Absolute paths of the files the diagram names (Code mode). */
  files: Map<string, string> | null;
  openCitation: ((n: number, trigger: HTMLElement) => void) | null;
}

function DiagramView({ spec, files, openCitation }: DiagramViewProps) {
  const reactId = useId();
  const prefix = `dg${reactId.replace(/[^a-zA-Z0-9]/g, '')}`;
  const drawing = useMemo(() => layoutDiagram(spec), [spec]);
  const markup = useMemo(() => diagramSvg(spec, drawing, { interactive: true, idPrefix: prefix }), [spec, drawing, prefix]);
  const description = useMemo(() => describeDiagram(spec), [spec]);
  const exported = useMemo(() => {
    const svg = diagramSvg(spec, drawing, { interactive: false, idPrefix: 'x' });
    // The description travels with the drawing, so a question about it reads the structure.
    return svg.replace('</title>', `</title><desc>${escapeXml(description)}</desc>`);
  }, [spec, drawing, description]);
  const getTarget = useCallback(() => svgTarget(exported, spec.title || 'Diagram'), [exported, spec.title]);
  const openFocus = useOpenFocus();
  const canvasRef = useRef<HTMLDivElement>(null);
  const [selected, setSelected] = useState<number | null>(null);

  useEffect(() => {
    setSelected(null);
  }, [spec]);

  // Selection is shown by `aria-pressed` on the node, without redrawing (keeps focus).
  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    canvas.querySelectorAll<SVGGElement>('[data-node]').forEach(el => {
      el.setAttribute('aria-pressed', String(Number(el.dataset.node) === selected));
    });
  }, [selected, markup]);

  const nodeFrom = (target: EventTarget | null): number | null => {
    if (!(target instanceof Element)) return null;
    const el = target.closest<SVGGElement>('[data-node]');
    if (!el || !canvasRef.current?.contains(el)) return null;
    const n = Number(el.dataset.node);
    return Number.isInteger(n) && n >= 0 && n < spec.nodes.length ? n : null;
  };

  const node = selected !== null ? spec.nodes[selected] : null;
  const file = node?.path && files ? files.get(node.path) ?? null : null;
  const hasDelta = spec.nodes.some(n => n.change) || spec.edges.some(e => e.change);
  const actionable = spec.nodes.some(n => n.path !== null || n.cite !== null);

  const askAboutNode = (trigger: HTMLElement) => {
    if (!openFocus || !node) return;
    const target = selectionTarget({ text: describeNode(spec, node.id), context: description, origin: 'answer' });
    if (target) openFocus(target, trigger);
  };

  return (
    <FocusFrame noun="diagram" getTarget={getTarget} className="my-4">
      <figure className="m-0 rounded-xl border border-shodh-border bg-shodh-surface overflow-hidden">
        {spec.title && (
          <figcaption className="px-4 pt-3 pr-32 text-[13px] font-semibold text-shodh-text">{spec.title}</figcaption>
        )}
        <div
          ref={canvasRef}
          className="px-4 py-3 overflow-x-auto scrollbar-thin flex justify-center [&_svg]:max-w-full [&_svg]:h-auto"
          onClick={e => {
            const n = nodeFrom(e.target);
            if (n !== null) setSelected(prev => (prev === n ? null : n));
          }}
          onKeyDown={e => {
            if (e.key === 'Escape' && selected !== null) {
              e.stopPropagation();
              setSelected(null);
              return;
            }
            if (e.key !== 'Enter' && e.key !== ' ') return;
            const n = nodeFrom(e.target);
            if (n === null) return;
            e.preventDefault();
            setSelected(prev => (prev === n ? null : n));
          }}
          // Markup built by diagramSvg from a checked spec; every string in it is escaped.
          dangerouslySetInnerHTML={{ __html: markup }}
        />
        <div role="status" aria-live="polite" className="empty:hidden border-t border-shodh-border-subtle px-4 py-2.5 flex flex-col gap-2">
          {node && (
            <>
              <div className="min-w-0 text-[12.5px] text-shodh-text-secondary">
                <span className="font-semibold text-shodh-text">{node.label}</span>
                {node.kind && <span className="ml-2 font-mono text-[11px] text-shodh-text-muted">{node.kind}</span>}
                {node.detail && <p className="mt-0.5 mb-0 text-shodh-text-tertiary">{node.detail}</p>}
              </div>
              <div className="flex flex-wrap items-center gap-2">
                {node.path && (
                  <button
                    type="button"
                    className={cn(ACTION, FOCUS_RING)}
                    disabled={!file}
                    title={file ?? 'This file is no longer in the code folder'}
                    onClick={() => {
                      if (file) publishTarget({ kind: 'document', path: file, page: null, passage: null });
                    }}
                  >
                    <FileCode2 className="w-3.5 h-3.5" aria-hidden="true" />
                    <span className="font-mono truncate max-w-[260px]">{node.path}</span>
                  </button>
                )}
                {node.cite !== null && openCitation && (
                  <button type="button" className={cn(ACTION, FOCUS_RING)} onClick={e => openCitation(node.cite as number, e.currentTarget)}>
                    <Quote className="w-3.5 h-3.5" aria-hidden="true" />
                    Show source {node.cite}
                  </button>
                )}
                {openFocus && (
                  <button type="button" className={cn(ACTION, FOCUS_RING)} onClick={e => askAboutNode(e.currentTarget)}>
                    <MessageSquareText className="w-3.5 h-3.5" aria-hidden="true" />
                    Ask about this
                  </button>
                )}
              </div>
            </>
          )}
          {!node && (hasDelta || actionable || drawing.loops.length > 0) && (
            <p className="m-0 text-[11.5px] text-shodh-text-muted">
              {[
                hasDelta ? '+ added · − removed (dashed) · Δ changed' : null,
                drawing.loops.length > 0 ? 'R reinforcing loop (even number of − links) · B balancing loop (odd)' : null,
                'Select a part to open its file or source, or to ask about it.',
              ].filter(Boolean).join('  ·  ')}
            </p>
          )}
        </div>
      </figure>
    </FocusFrame>
  );
}

export default DiagramBlock;
