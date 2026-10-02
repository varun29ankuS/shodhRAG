import React, { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { Loader2 } from 'lucide-react';
import { cn } from '../../../lib/utils';
import { readSourceTable, scrollBehavior, type SourceSheet } from './sourceAccess';
import { locateTableRows, type TableLocation } from './tableMatch';
import type { LocateResult } from './viewerTypes';
import { VIEWER_FOCUS_RING } from './viewerTypes';

const ROW_HEIGHT = 32;
const OVERSCAN = 12;
const INDEX_COLUMN_WIDTH = 64;
const SAMPLE_ROWS = 200;

function columnWidths(sheet: SourceSheet): number[] {
  return sheet.headers.map((header, col) => {
    let longest = header.length;
    const sample = Math.min(sheet.rows.length, SAMPLE_ROWS);
    for (let r = 0; r < sample; r += 1) {
      const value = sheet.rows[r][col];
      if (value && value.length > longest) longest = value.length;
    }
    return Math.min(320, Math.max(96, longest * 7.5 + 28));
  });
}

function describeRows(rows: readonly number[]): string {
  if (rows.length === 0) return '';
  const first = rows[0] + 1;
  const last = rows[rows.length - 1] + 1;
  return first === last ? `row ${first}` : `rows ${first}–${last}`;
}

interface TableViewerProps {
  filePath: string;
  passage: string;
  onLocate: (result: LocateResult) => void;
  onError: (error: unknown) => void;
}

/**
 * Spreadsheet / CSV viewer: sheet tabs, a virtualized grid (only visible rows
 * are in the DOM), and the rows the cited chunk came from highlighted.
 */
export function TableViewer({ filePath, passage, onLocate, onError }: TableViewerProps) {
  const scrollerRef = useRef<HTMLDivElement>(null);
  const tabRefs = useRef<(HTMLButtonElement | null)[]>([]);
  const [sheets, setSheets] = useState<SourceSheet[] | null>(null);
  const [active, setActive] = useState(0);
  const [location, setLocation] = useState<TableLocation | null>(null);
  const [scrollTop, setScrollTop] = useState(0);
  const [viewportHeight, setViewportHeight] = useState(600);
  const [scrollToken, setScrollToken] = useState(0);

  const onLocateRef = useRef(onLocate);
  const onErrorRef = useRef(onError);
  useEffect(() => {
    onLocateRef.current = onLocate;
    onErrorRef.current = onError;
  }, [onLocate, onError]);

  useEffect(() => {
    let cancelled = false;
    setSheets(null);
    setLocation(null);
    readSourceTable(filePath)
      .then(result => {
        if (!cancelled) setSheets(result);
      })
      .catch(error => {
        if (!cancelled) onErrorRef.current(error);
      });
    return () => {
      cancelled = true;
    };
  }, [filePath]);

  useEffect(() => {
    if (!sheets) return;
    if (sheets.length === 0) {
      onLocateRef.current({ status: 'notFound', message: 'This file contains no table data.' });
      return;
    }
    const found = passage.trim() ? locateTableRows(sheets, passage) : null;
    setLocation(found);
    setActive(found ? found.sheetIndex : 0);
    setScrollToken(t => t + 1);
    if (!found) {
      onLocateRef.current({
        status: 'notFound',
        message: 'The cited rows could not be matched in this file; showing the first sheet from the top.',
      });
      return;
    }
    const sheet = sheets[found.sheetIndex];
    const where = `${describeRows(found.rows)}${sheets.length > 1 ? ` of “${sheet.name}”` : ''}`;
    const beyond = sheet.truncated && found.rows.some(r => r >= sheet.rows.length);
    if (beyond) {
      onLocateRef.current({ status: 'approximate', message: `The cited ${where} lie beyond the ${sheet.rows.length.toLocaleString()} rows shown here.` });
    } else if (found.confirmed) {
      onLocateRef.current({ status: 'found', message: `Cited ${where} highlighted.` });
    } else {
      onLocateRef.current({ status: 'approximate', message: `Highlighted ${where} from the index; some values differ from the file as it is now.` });
    }
  }, [sheets, passage]);

  useEffect(() => {
    const scroller = scrollerRef.current;
    if (!scroller) return;
    const observer = new ResizeObserver(() => setViewportHeight(scroller.clientHeight));
    observer.observe(scroller);
    setViewportHeight(scroller.clientHeight);
    return () => observer.disconnect();
  }, [sheets]);

  // Bring the first cited row into view whenever the located sheet is shown.
  useLayoutEffect(() => {
    const scroller = scrollerRef.current;
    if (!scroller) return;
    if (location && location.sheetIndex === active && location.rows.length > 0) {
      const top = location.rows[0] * ROW_HEIGHT - scroller.clientHeight / 3;
      scroller.scrollTo({ top: Math.max(0, top), behavior: scrollBehavior() });
    } else {
      scroller.scrollTo({ top: 0 });
    }
  }, [active, location, scrollToken]);

  const sheet = sheets ? sheets[active] ?? null : null;
  const widths = useMemo(() => (sheet ? columnWidths(sheet) : []), [sheet]);
  const cited = useMemo(
    () => new Set(location && location.sheetIndex === active ? location.rows : []),
    [location, active],
  );

  if (!sheets) {
    return (
      <div className="flex-1 flex items-center justify-center gap-2 text-[13px] text-shodh-text-muted">
        <Loader2 className="w-4 h-4 animate-spin motion-reduce:animate-none" aria-hidden="true" />
        Loading table…
      </div>
    );
  }

  const onTabKeyDown = (e: React.KeyboardEvent<HTMLButtonElement>, index: number) => {
    let next = index;
    if (e.key === 'ArrowRight') next = (index + 1) % sheets.length;
    else if (e.key === 'ArrowLeft') next = (index - 1 + sheets.length) % sheets.length;
    else if (e.key === 'Home') next = 0;
    else if (e.key === 'End') next = sheets.length - 1;
    else return;
    e.preventDefault();
    setActive(next);
    tabRefs.current[next]?.focus();
  };

  const rowCount = sheet ? sheet.rows.length : 0;
  const first = Math.max(0, Math.floor(scrollTop / ROW_HEIGHT) - OVERSCAN);
  const last = Math.min(rowCount, Math.ceil((scrollTop + viewportHeight) / ROW_HEIGHT) + OVERSCAN);
  const tableWidth = INDEX_COLUMN_WIDTH + widths.reduce((sum, w) => sum + w, 0);
  const panelId = 'source-table-panel';

  return (
    <div className="flex-1 min-h-0 flex flex-col">
      {sheets.length > 1 && (
        <div role="tablist" aria-label="Sheets" className="flex gap-1 px-3 pt-2 border-b border-shodh-border-subtle overflow-x-auto scrollbar-thin">
          {sheets.map((s, index) => {
            const selected = index === active;
            const hasCitation = location?.sheetIndex === index;
            return (
              <button
                key={`${index}-${s.name}`}
                ref={el => {
                  tabRefs.current[index] = el;
                }}
                type="button"
                role="tab"
                id={`source-sheet-tab-${index}`}
                aria-selected={selected}
                aria-controls={panelId}
                tabIndex={selected ? 0 : -1}
                onClick={() => setActive(index)}
                onKeyDown={e => onTabKeyDown(e, index)}
                className={cn(
                  'shrink-0 h-8 px-3 -mb-px inline-flex items-center gap-1.5 rounded-t-lg border border-transparent text-[12.5px] transition-colors duration-micro',
                  selected
                    ? 'border-shodh-border-subtle border-b-shodh-surface bg-shodh-surface text-shodh-text font-semibold'
                    : 'text-shodh-text-muted hover:text-shodh-text hover:bg-shodh-raised',
                  VIEWER_FOCUS_RING,
                )}
              >
                {s.name}
                {hasCitation && (
                  <span className="w-1.5 h-1.5 rounded-full bg-shodh-warning" aria-label="contains cited rows" role="img" />
                )}
              </button>
            );
          })}
        </div>
      )}

      {sheet && sheet.truncated && (
        <p className="px-4 py-1.5 text-[12px] text-shodh-text-muted border-b border-shodh-border-subtle">
          Showing the first {sheet.rows.length.toLocaleString()} of {sheet.totalRows.toLocaleString()} rows.
        </p>
      )}

      <div
        ref={scrollerRef}
        id={panelId}
        role={sheets.length > 1 ? 'tabpanel' : undefined}
        aria-labelledby={sheets.length > 1 ? `source-sheet-tab-${active}` : undefined}
        tabIndex={0}
        onScroll={e => setScrollTop(e.currentTarget.scrollTop)}
        className={cn('flex-1 min-h-0 overflow-auto scrollbar-thin', VIEWER_FOCUS_RING)}
      >
        {sheet && (
          <table
            className="table-fixed border-separate border-spacing-0 text-[12.5px] text-shodh-text-secondary"
            style={{ width: tableWidth }}
            aria-label={sheet.name}
            aria-rowcount={rowCount + 1}
          >
            <colgroup>
              <col style={{ width: INDEX_COLUMN_WIDTH }} />
              {widths.map((w, i) => (
                <col key={i} style={{ width: w }} />
              ))}
            </colgroup>
            <thead className="sticky top-0 z-10">
              <tr aria-rowindex={1} style={{ height: ROW_HEIGHT }}>
                <th scope="col" className="px-2 text-right font-medium text-shodh-text-muted bg-shodh-raised border-b border-r border-shodh-border">
                  #
                </th>
                {sheet.headers.map((header, i) => (
                  <th
                    key={i}
                    scope="col"
                    title={header}
                    className="px-2.5 text-left font-semibold text-shodh-text bg-shodh-raised border-b border-r border-shodh-border truncate"
                  >
                    {header}
                  </th>
                ))}
              </tr>
            </thead>
            <tbody>
              {first > 0 && <tr aria-hidden="true" style={{ height: first * ROW_HEIGHT }} />}
              {sheet.rows.slice(first, last).map((row, offset) => {
                const index = first + offset;
                const isCited = cited.has(index);
                return (
                  <tr
                    key={index}
                    aria-rowindex={index + 2}
                    className={isCited ? 'source-row-cited' : undefined}
                    style={{ height: ROW_HEIGHT }}
                  >
                    <th scope="row" className="px-2 text-right font-normal tabular-nums text-shodh-text-muted border-b border-r border-shodh-border-subtle">
                      {isCited && <span className="sr-only">Cited row </span>}
                      {index + 1}
                    </th>
                    {sheet.headers.map((_, col) => {
                      const value = row[col] ?? '';
                      return (
                        <td key={col} title={value} className="px-2.5 border-b border-r border-shodh-border-subtle truncate">
                          {value}
                        </td>
                      );
                    })}
                  </tr>
                );
              })}
              {last < rowCount && <tr aria-hidden="true" style={{ height: (rowCount - last) * ROW_HEIGHT }} />}
            </tbody>
          </table>
        )}
      </div>
    </div>
  );
}
