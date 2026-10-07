import { useCallback, useEffect, useId, useRef, useState } from 'react';
import { AlertTriangle, Wrench } from 'lucide-react';
import { cn } from '../../lib/utils';
import { errorText, toolsApi } from './api';
import type { ChatToolsView, ToolMode } from './api';
import { chipLabel } from './format';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1 focus-visible:ring-offset-shodh-surface';

interface ToolsChipProps {
  workspaceId: string | null;
  mode: ToolMode;
  /** Re-read after each answer (a server may have started or stopped). */
  answerRunning: boolean;
  /** Open Settings → Tools & connections. */
  onOpenSettings: () => void;
}

/**
 * "2 MCP · 1 skill · 31 tools" next to the mode switch: what this chat can
 * use, computed exactly as the agent session computes it. Click for the list.
 */
export function ToolsChip({ workspaceId, mode, answerRunning, onOpenSettings }: ToolsChipProps) {
  const [view, setView] = useState<ChatToolsView | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [open, setOpen] = useState(false);
  const buttonRef = useRef<HTMLButtonElement>(null);
  const popoverRef = useRef<HTMLDivElement>(null);
  const popoverId = useId();
  const headingId = useId();

  useEffect(() => {
    if (answerRunning) return;
    let active = true;
    toolsApi
      .forChat(workspaceId, mode)
      .then(next => {
        if (!active) return;
        setView(next);
        setError(null);
      })
      .catch(e => {
        if (active) setError(errorText(e));
      });
    return () => {
      active = false;
    };
  }, [workspaceId, mode, answerRunning]);

  const close = useCallback((refocus: boolean) => {
    setOpen(false);
    if (refocus) buttonRef.current?.focus();
  }, []);

  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.stopPropagation();
        close(true);
      }
    };
    const onPointer = (e: PointerEvent) => {
      const target = e.target as Node;
      if (!popoverRef.current?.contains(target) && !buttonRef.current?.contains(target)) close(false);
    };
    window.addEventListener('keydown', onKey, true);
    window.addEventListener('pointerdown', onPointer);
    return () => {
      window.removeEventListener('keydown', onKey, true);
      window.removeEventListener('pointerdown', onPointer);
    };
  }, [open, close]);

  if (!view && !error) return null;
  const label = view ? chipLabel(view) : 'Tools unavailable';
  const failing = view?.servers.filter(s => s.error !== null) ?? [];

  return (
    <div className="relative shrink-0">
      <button
        ref={buttonRef}
        type="button"
        onClick={() => setOpen(o => !o)}
        aria-haspopup="dialog"
        aria-expanded={open}
        aria-controls={open ? popoverId : undefined}
        title="Tools this chat can use"
        className={cn(
          'inline-flex items-center gap-1 h-[26px] px-2 rounded-full text-[11.5px] font-medium border transition-colors duration-micro',
          view?.many || failing.length > 0 || error
            ? 'border-shodh-warning text-shodh-text-secondary hover:bg-shodh-raised'
            : 'border-shodh-border text-shodh-text-muted hover:text-shodh-text hover:bg-shodh-raised',
          FOCUS_RING,
        )}
      >
        {view?.many || failing.length > 0 || error ? (
          <AlertTriangle className="w-3 h-3 text-shodh-warning" aria-hidden="true" />
        ) : (
          <Wrench className="w-3 h-3" aria-hidden="true" />
        )}
        <span className="whitespace-nowrap">{label}</span>
      </button>
      {open && (
        <div
          ref={popoverRef}
          id={popoverId}
          role="dialog"
          aria-labelledby={headingId}
          className="absolute bottom-full left-0 mb-2 z-40 w-[340px] max-h-[420px] overflow-y-auto scrollbar-thin rounded-[14px] border border-shodh-border bg-shodh-raised p-3.5 shadow-[0_12px_40px_rgba(0,0,0,0.35)] flex flex-col gap-3"
        >
          <h2 id={headingId} className="m-0 text-[13px] font-semibold text-shodh-text">
            This chat can use
          </h2>
          {error && <p className="m-0 text-[12.5px] text-shodh-error">{error}</p>}
          {view && (
            <>
              {view.many && (
                <p className="m-0 text-[12px] text-shodh-text-secondary bg-shodh-warning-soft rounded-lg px-2.5 py-2">
                  {view.total} tools is a lot: models choose less well with more than 40. Turn off servers or tools you
                  do not need here.
                </p>
              )}
              <p className="m-0 text-[12.5px] text-shodh-text-secondary">
                {view.builtin} built-in {mode === 'code' ? 'Code mode' : 'Research'} tools
              </p>
              {view.servers.length > 0 && (
                <section aria-label="MCP servers" className="flex flex-col gap-1.5">
                  <h3 className="m-0 text-[11.5px] font-semibold uppercase tracking-wide text-shodh-text-faint">
                    MCP servers
                  </h3>
                  <ul className="m-0 p-0 list-none flex flex-col gap-1.5">
                    {view.servers.map(server => (
                      <li key={`${server.scope}-${server.name}`} className="text-[12.5px]">
                        <span className="font-medium text-shodh-text">{server.name}</span>
                        {server.scope === 'workspace' && <span className="text-shodh-text-faint"> · workspace</span>}
                        {server.error ? (
                          <p className="m-0 text-shodh-error">{server.error}</p>
                        ) : (
                          <p className="m-0 text-shodh-text-muted break-words">
                            {server.tools.length === 0 ? 'No tools enabled' : server.tools.join(', ')}
                          </p>
                        )}
                      </li>
                    ))}
                  </ul>
                </section>
              )}
              {view.skills.length > 0 && (
                <section aria-label="Skills" className="flex flex-col gap-1.5">
                  <h3 className="m-0 text-[11.5px] font-semibold uppercase tracking-wide text-shodh-text-faint">Skills</h3>
                  <ul className="m-0 p-0 list-none flex flex-col gap-1">
                    {view.skills.map(skill => (
                      <li key={skill.name} className="text-[12.5px]">
                        <span className="font-medium text-shodh-text">{skill.name}</span>
                        <span className="text-shodh-text-muted"> · {skill.description}</span>
                      </li>
                    ))}
                  </ul>
                </section>
              )}
            </>
          )}
          <button
            type="button"
            onClick={() => {
              close(false);
              onOpenSettings();
            }}
            className={cn('self-start text-[12.5px] font-medium text-shodh-accent-text hover:underline rounded', FOCUS_RING)}
          >
            Manage tools and connections
          </button>
        </div>
      )}
    </div>
  );
}

export default ToolsChip;
