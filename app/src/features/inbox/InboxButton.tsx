import React, { useCallback, useEffect, useId, useMemo, useReducer, useRef, useState, useSyncExternalStore } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { Bell, Check, ExternalLink, Inbox as InboxIcon, RotateCcw, X } from 'lucide-react';
import { cn } from '../../lib/utils';
import { notify } from '../../lib/notify';
import { removeWithUndo, undoLast } from '../../lib/undoToast';
import { relativeTime } from '../../utils/time';
import { InfoPopover } from '../../components/ui/InfoPopover';
import { acceptSuggestion, errorText, rejectSuggestion } from '../memory/api';
import { pendingRejections } from '../memory/rejecting';
import { parseTarget, publishTarget } from '../agent/navigation';
import { announceApproval } from './approvalBus';
import { normalizeViewTab } from '../../lib/viewTabs';
import type { ViewTab } from '../../lib/viewTabs';
import { requestSettingsSection } from '../memory/navigation';
import { parseConnectorTarget, requestConnectorPage } from '../connectors/navigation';
import {
  CHOICE_LABEL,
  EMPTY_COPY,
  INBOX_TABS,
  STATUS_LABEL,
  STATUS_TONE,
  TAB_LABEL,
  TAB_RULES,
  actionsFor,
  focusZone,
  inboxCounts,
  inboxCommand,
  keyAction,
  keepSelection,
  moveSelection,
  panelNav,
  panelState,
  parseItems,
  pointerDownCloses,
  requestGate,
  retryButton,
  settingsSectionOf,
  tabItems,
} from './model';
import type { InboxChoice, InboxItem, InboxTab, PrimaryAction, RequestGate } from './model';
import { repeatDecide, repeatGet, repeatSet } from '../tasks/repeatApi';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1 focus-visible:ring-offset-shodh-surface';
// Labels never wrap: a row of actions stays one line (UI audit rule 7).
const ACTION =
  'h-7 px-2 shrink-0 whitespace-nowrap inline-flex items-center gap-1 rounded-md text-caption font-medium transition-colors duration-micro disabled:opacity-50';

const PRIMARY_LABEL: Record<PrimaryAction, string> = { approve: 'Approve', accept: 'Accept', install: 'Install' };

/** Opened from the command palette ("Inbox"). */
export const OPEN_INBOX_EVENT = 'shodh:open-inbox';

interface InboxButtonProps {
  onNavigate: (view: ViewTab) => void;
  onOpenConversation: (id: string) => void;
}

function str(value: unknown): string | null {
  return typeof value === 'string' && value.length > 0 ? value : null;
}

function targetElement(target: EventTarget | null): Element | null {
  return target instanceof Element ? target : null;
}

/**
 * The bell in the header and its Inbox, in two tabs: Attention (what waits on the
 * user, and what stays stopped until they fix it; the bell counts these) and Info
 * (everything else). J/K move, Enter opens, A approves or accepts, D denies or
 * dismisses, U undoes the last dismissal.
 */
export function InboxButton({ onNavigate, onOpenConversation }: InboxButtonProps) {
  const [{ open, tab }, nav] = useReducer(panelNav, { open: false, tab: 'attention' });
  const [items, setItems] = useState<InboxItem[]>([]);
  const [retrying, setRetrying] = useState(false);
  const gateRef = useRef<RequestGate>(requestGate());
  const sectionRef = useRef<HTMLElement>(null);
  const [loaded, setLoaded] = useState(false);
  const [hidden, setHidden] = useState<ReadonlySet<string>>(() => new Set());
  const [selected, setSelected] = useState<Record<InboxTab, number>>({ attention: 0, info: 0 });
  const [busy, setBusy] = useState<string | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const rootRef = useRef<HTMLDivElement>(null);
  const buttonRef = useRef<HTMLButtonElement>(null);
  const rowRefs = useRef(new Map<string, HTMLButtonElement>());
  const tabRefs = useRef(new Map<InboxTab, HTMLButtonElement>());
  // A row takes focus when the panel opens and on J/K, never on a tab switch.
  const focusRowRef = useRef(false);
  const panelId = useId();
  const headingId = useId();
  const tabsId = useId();
  const tabId = (t: InboxTab) => `${tabsId}-${t}`;
  const tabPanelId = `${tabsId}-panel`;

  // Reads overlap (several change events at once): only the latest answer is applied.
  const load = useCallback(async () => {
    const gate = gateRef.current;
    const ticket = gate.begin();
    try {
      const listed = parseItems(await invoke<unknown>('inbox_list'));
      if (!gate.isLatest(ticket)) return;
      setItems(listed);
      setLoadError(null);
      setLoaded(true);
    } catch (err) {
      if (!gate.isLatest(ticket)) return;
      setLoadError(errorText(err));
      setLoaded(true);
    }
  }, []);

  const retry = async () => {
    setRetrying(true);
    try {
      await load();
    } finally {
      setRetrying(false);
    }
  };

  useEffect(() => {
    void load();
    // A task done or deleted settles its reminder (`inbox_list` drops it).
    const unlisten = [
      listen('inbox-changed', () => void load()),
      listen('memory-suggestions-changed', () => void load()),
      listen('calendar-changed', () => void load()),
    ];
    const openFromPalette = () => nav({ type: 'open' });
    window.addEventListener(OPEN_INBOX_EVENT, openFromPalette);
    return () => {
      window.removeEventListener(OPEN_INBOX_EVENT, openFromPalette);
      for (const off of unlisten) void off.then(fn => fn());
    };
  }, [load]);

  // Opening lands on Attention (panelNav) and focuses its selected row.
  useEffect(() => {
    if (open) focusRowRef.current = true;
  }, [open]);

  // Suggestions being rejected in Settings → Memory are hidden here too.
  const rejecting = useSyncExternalStore(pendingRejections.subscribe, pendingRejections.get);
  const visible = useMemo(
    () =>
      items.filter(i => {
        if (hidden.has(i.id)) return false;
        const suggestionId = i.kind === 'memory' ? str(i.data.suggestionId) : null;
        return !(suggestionId && rejecting.has(suggestionId));
      }),
    [items, hidden, rejecting],
  );
  const list = useMemo(() => tabItems(visible, tab), [visible, tab]);
  const infoHasItems = useMemo(() => tabItems(visible, 'info').length > 0, [visible]);
  const counts = inboxCounts({ loaded, error: loadError, items: visible });
  const current = list[selected[tab]] ?? null;
  const state = panelState({ loaded, error: loadError, count: list.length });

  // The selection follows its item when the list changes, in each tab on its own.
  const selectedIdRef = useRef<Record<InboxTab, string | null>>({ attention: null, info: null });
  useEffect(() => {
    setSelected(prev => {
      const index = keepSelection(selectedIdRef.current[tab], prev[tab], list);
      return index === prev[tab] ? prev : { ...prev, [tab]: index };
    });
  }, [list, tab]);
  useEffect(() => {
    selectedIdRef.current[tab] = current?.id ?? null;
  }, [current, tab]);

  // Close on a click outside; a click in the "Why is this here?" pop-up (a portal) is not outside.
  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (pointerDownCloses<Element>(targetElement(e.target), rootRef.current)) nav({ type: 'close' });
    };
    document.addEventListener('mousedown', onDown);
    return () => document.removeEventListener('mousedown', onDown);
  }, [open]);

  useEffect(() => {
    if (!open || !current) return;
    // Also when the focused row went away (dismissed, answered): focus stays in the list.
    if (focusRowRef.current || document.activeElement === document.body) {
      focusRowRef.current = false;
      rowRefs.current.get(current.id)?.focus();
    }
  }, [open, current?.id]);

  const hide = (id: string, on: boolean) =>
    setHidden(prev => {
      const next = new Set(prev);
      if (on) next.add(id);
      else next.delete(id);
      return next;
    });

  const openItem = (item: InboxItem) => {
    const link = item.link;
    if (!link) return;
    const view = normalizeViewTab(link.view);
    const target = parseTarget(link.target);
    nav({ type: 'close' });
    if (target?.kind === 'conversation') {
      onOpenConversation(target.conversationId);
      return;
    }
    if (target) publishTarget(target);
    // A connector's item opens its page, at the tab it names.
    const connectorPage = view === 'connectors' ? parseConnectorTarget(link.target) : null;
    if (connectorPage) requestConnectorPage(connectorPage);
    const section = settingsSectionOf(item);
    if (section) requestSettingsSection(section);
    if (view) onNavigate(view);
  };

  const run = async (item: InboxItem, action: () => Promise<unknown>, failure: string) => {
    setBusy(item.id);
    try {
      await action();
      await load();
    } catch (err) {
      notify.error(failure, { description: errorText(err) });
    } finally {
      setBusy(null);
    }
  };

  // Answer an approval, and let the chat that shows the step know.
  const decide = (item: InboxItem, approved: boolean) => {
    const sessionId = str(item.data.sessionId);
    const stepId = str(item.data.stepId);
    if (!sessionId || !stepId) return;
    void run(item, async () => {
      await invoke('agent_approve', { sessionId, stepId, approved });
      announceApproval({ sessionId, stepId, approved });
    }, approved ? 'The step was not approved' : 'The step was not denied');
  };

  const primary = (item: InboxItem) => {
    const { primary: action } = actionsFor(item);
    if (action === 'approve') {
      decide(item, true);
    } else if (action === 'accept') {
      const id = str(item.data.suggestionId);
      if (id) void run(item, () => acceptSuggestion(id), 'The suggestion was not accepted');
    } else if (action === 'install') {
      // The scan reader is installed with the search models: only its files are missing.
      void run(item, () => invoke('install_search_models'), 'The scan reader was not installed');
    }
  };

  // Answer a repeating task's question, or resume one on hold (saving its repeat again).
  const choose = (item: InboxItem, choice: InboxChoice) => {
    const jobId = str(item.data.jobId);
    const taskId = str(item.data.taskId);
    if (choice === 'resume') {
      if (!taskId) return;
      void run(item, async () => {
        const view = await repeatGet(taskId);
        if (view) await repeatSet(taskId, view.schedule);
      }, 'The repeat was not resumed');
      return;
    }
    if (!jobId) return;
    void run(item, () => repeatDecide(jobId, choice), 'The answer was not saved');
  };

  const secondary = (item: InboxItem) => {
    const { secondary: action } = actionsFor(item);
    if (action === 'deny') {
      decide(item, false);
      return;
    }
    if (action !== 'dismiss') return;
    const suggestionId = item.kind === 'memory' ? str(item.data.suggestionId) : null;
    // A dismissed suggestion is still pending until the undo window ends: Settings →
    // Memory's batch accept leaves it out meanwhile (shared ids).
    const rejection = suggestionId ? pendingRejections.around(suggestionId, () => rejectSuggestion(suggestionId)) : null;
    removeWithUndo({
      message: item.kind === 'memory' ? 'Suggestion dismissed' : 'Removed from the Inbox',
      description: item.title,
      hide: () => {
        hide(item.id, true);
        rejection?.hide();
      },
      restore: () => {
        hide(item.id, false);
        rejection?.restore();
      },
      commit: async () => {
        if (rejection) await rejection.commit();
        else await invoke('inbox_dismiss', { id: item.id });
        await load();
        hide(item.id, false);
      },
      onError: err => notify.error('It was not dismissed', { description: errorText(err) }),
    });
  };

  const showTab = (next: InboxTab, focus: boolean) => {
    nav({ type: 'show', tab: next });
    if (focus) tabRefs.current.get(next)?.focus();
  };

  // The tab list's own keys (ARIA tabs): Left and Right move between the two tabs.
  const onTabKeyDown = (e: React.KeyboardEvent) => {
    if (e.key !== 'ArrowLeft' && e.key !== 'ArrowRight') return;
    e.preventDefault();
    e.stopPropagation();
    const at = INBOX_TABS.indexOf(tab);
    const step = e.key === 'ArrowRight' ? 1 : -1;
    showTab(INBOX_TABS[(at + step + INBOX_TABS.length) % INBOX_TABS.length], true);
  };

  const onKeyDown = (e: React.KeyboardEvent) => {
    // Keys in the "Why is this here?" pop-up (a portal, bubbling here) are its own.
    const zone = focusZone<Element>(targetElement(e.target), sectionRef.current);
    if (zone === 'outside') return;
    if (e.key === 'Escape') {
      e.preventDefault();
      nav({ type: 'close' });
      buttonRef.current?.focus();
      return;
    }
    const target = e.target as HTMLElement;
    const typed = inboxCommand(e, target.tagName === 'INPUT' || target.tagName === 'TEXTAREA');
    const command = typed === null ? null : keyAction(typed, zone);
    if (!command) return;
    e.preventDefault();
    if (command === 'next' || command === 'previous') {
      focusRowRef.current = true;
      setSelected(prev => ({ ...prev, [tab]: moveSelection(prev[tab], list.length, command === 'next' ? 1 : -1) }));
    } else if (command === 'undo') {
      if (!undoLast()) notify.info('Nothing to undo');
    } else if (current && busy === null) {
      if (command === 'open') openItem(current);
      else if (command === 'primary') primary(current);
      else secondary(current);
    }
  };

  const retryAction = retryButton(retrying);

  return (
    <div className="relative" ref={rootRef}>
      <button
        ref={buttonRef}
        type="button"
        onClick={() => nav({ type: 'toggle' })}
        aria-expanded={open}
        aria-controls={open ? panelId : undefined}
        aria-label={counts.bellAria}
        title="Inbox"
        className={cn(
          'relative w-7 h-7 rounded-md inline-flex items-center justify-center text-shodh-text-secondary hover:bg-shodh-raised hover:text-shodh-text transition-colors duration-micro',
          FOCUS_RING,
        )}
      >
        <Bell className="w-4 h-4" aria-hidden="true" />
        {counts.bell !== null && (
          <span
            aria-hidden="true"
            className="absolute -top-1.5 -right-1.5 min-w-[18px] h-[18px] px-1 rounded-full bg-shodh-warning-soft text-shodh-text ring-1 ring-shodh-warning text-caption font-bold leading-[18px] text-center tabular-nums whitespace-nowrap"
          >
            {counts.bell}
          </span>
        )}
      </button>

      {open && (
        <section
          ref={sectionRef}
          id={panelId}
          aria-labelledby={headingId}
          onKeyDown={onKeyDown}
          className="absolute right-0 top-9 z-50 w-[400px] max-w-[calc(100vw-24px)] rounded-xl border border-shodh-border bg-shodh-surface shadow-lg overflow-hidden flex flex-col"
        >
          <header className="flex items-center justify-between gap-2 px-3.5 pt-2.5">
            <h2 id={headingId} className="m-0 text-small font-semibold text-shodh-text">Inbox</h2>
            <InfoPopover label="Why is this here?" title="Why is this here?">
              {INBOX_TABS.map(t => (
                <p key={t} className="m-0 mt-2 text-small text-shodh-text-secondary">{TAB_RULES[t]}</p>
              ))}
            </InfoPopover>
          </header>

          <div role="tablist" aria-label="Inbox tabs" className="grid grid-cols-2 gap-1 px-2 pt-2 pb-1.5 border-b border-shodh-border-subtle">
            {INBOX_TABS.map(t => {
              const active = tab === t;
              return (
                <button
                  key={t}
                  ref={el => {
                    if (el) tabRefs.current.set(t, el);
                    else tabRefs.current.delete(t);
                  }}
                  id={tabId(t)}
                  type="button"
                  role="tab"
                  aria-selected={active}
                  aria-controls={tabPanelId}
                  tabIndex={active ? 0 : -1}
                  onClick={() => showTab(t, false)}
                  onKeyDown={onTabKeyDown}
                  className={cn(
                    'h-8 px-2 inline-flex items-center justify-center gap-1.5 rounded-md text-small font-medium whitespace-nowrap transition-colors duration-micro',
                    active
                      ? 'bg-shodh-raised text-shodh-text'
                      : 'text-shodh-text-secondary hover:bg-shodh-raised hover:text-shodh-text',
                    FOCUS_RING,
                  )}
                >
                  {TAB_LABEL[t]}
                  {/* Only Attention has a count: Info is finished work, not a to-do list. */}
                  {t === 'attention' && counts.pill !== null && (
                    <span
                      className={cn(
                        'min-w-[28px] h-[18px] px-1 rounded-full text-caption font-bold leading-[18px] text-center tabular-nums',
                        counts.bell !== null ? 'bg-shodh-warning-soft text-shodh-text ring-1 ring-shodh-warning' : 'bg-shodh-raised-2 text-shodh-text-muted',
                      )}
                    >
                      {counts.pill}
                    </span>
                  )}
                </button>
              );
            })}
          </div>

          <div id={tabPanelId} role="tabpanel" aria-labelledby={tabId(tab)} aria-busy={state === 'loading' || undefined}>
            {state === 'list' && loadError && (
              <div role="alert" className="mx-2 mt-1.5 px-2.5 py-1.5 flex items-center gap-2 rounded-md border border-shodh-border bg-shodh-raised text-caption text-shodh-error">
                <span className="flex-1 min-w-0 truncate" title={loadError}>{`Couldn’t refresh the Inbox: ${loadError}`}</span>
                <button type="button" onClick={() => void retry()} disabled={retryAction.disabled} className={cn(ACTION, 'border border-shodh-border text-shodh-text-secondary hover:bg-shodh-raised-2 hover:text-shodh-text', FOCUS_RING)}>
                  <RotateCcw className="w-3.5 h-3.5" aria-hidden="true" />
                  {retryAction.label}
                </button>
              </div>
            )}
            {state === 'loading' ? (
              <p className="m-0 px-3.5 py-8 text-center text-small text-shodh-text-muted">Loading the Inbox…</p>
            ) : state === 'error' ? (
              <div role="alert" className="px-3.5 py-6 flex flex-col items-center gap-2 text-center">
                <p className="m-0 text-small text-shodh-error break-words">{`Couldn’t load the Inbox: ${loadError}`}</p>
                <button type="button" onClick={() => void retry()} disabled={retryAction.disabled} className={cn(ACTION, 'border border-shodh-border text-shodh-text-secondary hover:bg-shodh-raised-2 hover:text-shodh-text', FOCUS_RING)}>
                  <RotateCcw className="w-3.5 h-3.5" aria-hidden="true" />
                  {retryAction.label}
                </button>
              </div>
            ) : state === 'empty' ? (
              <div className="px-3.5 py-8 flex flex-col items-center gap-1.5 text-center">
                <InboxIcon className="w-5 h-5 text-shodh-text-faint" aria-hidden="true" />
                <p className="m-0 text-small text-shodh-text-secondary">{EMPTY_COPY[tab].title}</p>
                <p className="m-0 text-caption text-shodh-text-muted">{EMPTY_COPY[tab].detail}</p>
                {tab === 'attention' && infoHasItems && (
                  <button
                    type="button"
                    onClick={() => showTab('info', true)}
                    className={cn(ACTION, 'mt-1.5 border border-shodh-border text-shodh-text-secondary hover:bg-shodh-raised-2 hover:text-shodh-text', FOCUS_RING)}
                  >
                    Show Info
                  </button>
                )}
              </div>
            ) : (
              <ul data-inbox-list="" className="m-0 p-1 list-none max-h-[420px] overflow-y-auto" aria-label={`${TAB_LABEL[tab]} items`}>
                {list.map((item, index) => {
                  const tone = STATUS_TONE[item.status];
                  const actions = actionsFor(item);
                  const isSelected = index === selected[tab];
                  const disabled = busy === item.id;
                  // An item with one-click answers opens from its row: the answers need the width.
                  const showOpen = item.link !== null && !actions.choices;
                  return (
                    <li
                      key={item.id}
                      className={cn('rounded-lg px-2.5 py-2 flex flex-col gap-1.5', isSelected && 'bg-shodh-raised')}
                    >
                      <button
                        ref={el => {
                          if (el) rowRefs.current.set(item.id, el);
                          else rowRefs.current.delete(item.id);
                        }}
                        type="button"
                        data-inbox-row=""
                        tabIndex={isSelected ? 0 : -1}
                        aria-current={isSelected ? 'true' : undefined}
                        onFocus={() => setSelected(prev => (prev[tab] === index ? prev : { ...prev, [tab]: index }))}
                        onClick={() => openItem(item)}
                        className={cn('text-left flex items-start gap-2 rounded-md min-w-0', FOCUS_RING)}
                      >
                        <span className={cn('mt-[5px] w-2 h-2 rounded-full shrink-0', tone.dot)} aria-hidden="true" />
                        <span className="min-w-0 flex-1 flex flex-col gap-0.5">
                          <span className="text-small font-medium text-shodh-text break-words">{item.title}</span>
                          <span className="text-caption text-shodh-text-muted break-words">
                            <span className={tone.text}>{STATUS_LABEL[item.status]}</span>
                            {item.detail ? ` · ${item.detail}` : ''}
                            {item.updatedAt ? ` · ${relativeTime(item.updatedAt)}` : ''}
                          </span>
                        </span>
                      </button>
                      {(actions.primary || actions.secondary || actions.choices || showOpen) && (
                        <div className="flex flex-nowrap items-center gap-1.5 pl-4 min-w-0">
                          {actions.choices?.map((choice, i) => (
                            <button
                              key={choice}
                              type="button"
                              tabIndex={isSelected ? 0 : -1}
                              disabled={disabled}
                              onClick={() => choose(item, choice)}
                              className={cn(
                                ACTION,
                                i === 0
                                  ? 'bg-shodh-accent text-shodh-on-accent hover:bg-shodh-accent-hover'
                                  : 'border border-shodh-border text-shodh-text-secondary hover:bg-shodh-raised-2 hover:text-shodh-text',
                                FOCUS_RING,
                              )}
                            >
                              {CHOICE_LABEL[choice]}
                            </button>
                          ))}
                          {actions.primary && (
                            <button
                              type="button"
                              tabIndex={isSelected ? 0 : -1}
                              disabled={disabled}
                              onClick={() => primary(item)}
                              className={cn(ACTION, 'bg-shodh-accent text-shodh-on-accent hover:bg-shodh-accent-hover', FOCUS_RING)}
                            >
                              <Check className="w-3.5 h-3.5" aria-hidden="true" />
                              {PRIMARY_LABEL[actions.primary]}
                            </button>
                          )}
                          {actions.secondary && (
                            <button
                              type="button"
                              tabIndex={isSelected ? 0 : -1}
                              disabled={disabled}
                              onClick={() => secondary(item)}
                              className={cn(ACTION, 'border border-shodh-border text-shodh-text-secondary hover:bg-shodh-raised-2 hover:text-shodh-text', FOCUS_RING)}
                            >
                              <X className="w-3.5 h-3.5" aria-hidden="true" />
                              {actions.secondary === 'deny' ? 'Deny' : 'Dismiss'}
                            </button>
                          )}
                          {showOpen && (
                            <button
                              type="button"
                              tabIndex={isSelected ? 0 : -1}
                              onClick={() => openItem(item)}
                              className={cn(ACTION, 'text-shodh-text-secondary hover:bg-shodh-raised-2 hover:text-shodh-text', FOCUS_RING)}
                            >
                              <ExternalLink className="w-3.5 h-3.5" aria-hidden="true" />
                              Open
                            </button>
                          )}
                        </div>
                      )}
                    </li>
                  );
                })}
              </ul>
            )}
          </div>

          <footer className="px-3.5 py-2 border-t border-shodh-border-subtle">
            <p className="m-0 text-caption text-shodh-text-muted whitespace-nowrap">
              <kbd>J</kbd>/<kbd>K</kbd> move · <kbd>A</kbd> approve · <kbd>D</kbd> deny or dismiss · <kbd>U</kbd> undo
            </p>
          </footer>
        </section>
      )}
    </div>
  );
}
