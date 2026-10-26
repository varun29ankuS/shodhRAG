import React, { useCallback, useEffect, useRef, useState } from 'react';
import { cn } from '../../lib/utils';
import { CONNECTORS, type ConnectorInfo } from './registry';
import {
  OPEN_CONNECTOR_PAGE,
  escapeGoesBack,
  lastConnectorPage,
  rememberConnectorPage,
  takeRequestedConnectorPage,
  type ConnectorPage,
} from './navigation';
import { slackPill, tileAction, type Pill, type SlackTabId } from './slackModel';
import { SlackMark } from './SlackLogo';
import { SlackPage } from './SlackPage';
import { EMPTY_WIZARD, type WizardState } from './SlackSetup';
import { telegramPill, telegramTileAction, type TelegramTabId } from './telegramModel';
import { TelegramMark } from './TelegramLogo';
import { TelegramPage } from './TelegramPage';
import { EMPTY_TELEGRAM_WIZARD, type TelegramWizardState } from './TelegramSetup';
import { FOCUS_RING, Skeleton, StatusPill } from './ui';
import { useSlackStatus } from './useSlackStatus';
import { useTelegramStatus } from './useTelegramStatus';

/** A connector in the gallery: its mark, name, one line, its state and one action. */
function ConnectorTile({
  info,
  mark,
  pill,
  action,
  loadError,
  onOpen,
}: {
  info: ConnectorInfo;
  mark: React.ReactNode;
  pill: Pill | null;
  action: 'Connect' | 'Manage' | 'Open';
  loadError: string | null;
  onOpen: () => void;
}) {
  const state = pill ? `${pill.label}${pill.reason ? `, ${pill.reason}` : ''}` : loadError ? 'could not be loaded' : 'loading';
  return (
    <li>
      <button
        type="button"
        data-connector={info.id}
        onClick={onOpen}
        aria-label={`${info.name}: ${state}. ${action}`}
        className={cn(
          'group w-full h-full flex flex-col gap-4 p-5 rounded-xl border border-shodh-border bg-shodh-surface text-left hover:border-shodh-border-strong hover:bg-shodh-raised transition-colors duration-micro',
          FOCUS_RING,
        )}
      >
        <span className="flex items-start gap-3.5">
          {mark}
          <span className="min-w-0 flex-1 flex flex-col gap-1">
            <span className="text-[15px] font-semibold text-shodh-text">{info.name}</span>
            <span className="text-[12.5px] leading-[1.5] text-shodh-text-secondary">{info.summary}</span>
          </span>
        </span>
        <span className="mt-auto flex items-center gap-3 min-w-0">
          <span className="min-w-0 flex-1 flex flex-col items-start gap-0.5">
            {pill ? (
              <StatusPill pill={pill} />
            ) : loadError ? (
              <span className="text-[12px] text-shodh-error">Could not be loaded</span>
            ) : (
              <Skeleton className="h-6 w-28 rounded-full" />
            )}
            {pill?.team && <span className="max-w-full text-[12px] text-shodh-text-muted truncate">{pill.team}</span>}
          </span>
          <span
            aria-hidden="true"
            className={cn(
              'shrink-0 h-8 px-3 inline-flex items-center rounded-lg text-[12.5px] font-semibold transition-colors duration-micro',
              action === 'Connect'
                ? 'bg-shodh-accent text-shodh-on-accent group-hover:bg-shodh-accent-hover'
                : 'border border-shodh-border text-shodh-text-secondary group-hover:text-shodh-text',
            )}
          >
            {action}
          </span>
        </span>
      </button>
    </li>
  );
}

/**
 * Connectors: a gallery of tiles, one per connector, and each connector's page. The page
 * shown last is remembered for this session; an Inbox item opens a page at its tab.
 */
export default function ConnectorsView() {
  const slack = useSlackStatus();
  const telegram = useTelegramStatus();
  const [telegramWizard, setTelegramWizard] = useState<TelegramWizardState>(EMPTY_TELEGRAM_WIZARD);
  const [page, setPage] = useState<ConnectorPage | null>(() => takeRequestedConnectorPage() ?? lastConnectorPage());
  const [wizard, setWizard] = useState<WizardState>(EMPTY_WIZARD);
  const rootRef = useRef<HTMLDivElement>(null);
  const tilesRef = useRef<HTMLUListElement>(null);

  const show = useCallback((next: ConnectorPage | null) => {
    setPage(next);
    rememberConnectorPage(next);
  }, []);

  useEffect(() => {
    const onOpen = () => {
      const requested = takeRequestedConnectorPage();
      if (requested) {
        show(requested);
        rootRef.current?.scrollTo({ top: 0 });
      }
    };
    window.addEventListener(OPEN_CONNECTOR_PAGE, onOpen);
    return () => window.removeEventListener(OPEN_CONNECTOR_PAGE, onOpen);
  }, [show]);

  const back = useCallback(() => {
    const from = page?.connector ?? null;
    show(null);
    // Focus goes back to the tile that was opened.
    window.requestAnimationFrame(() => {
      const tiles = tilesRef.current;
      const tile = from ? tiles?.querySelector<HTMLButtonElement>(`button[data-connector="${from}"]`) : null;
      (tile ?? tiles?.querySelector<HTMLButtonElement>('button'))?.focus();
    });
  }, [show, page]);

  // Escape on a page goes back to the gallery, unless a field, menu or confirm took it.
  const onKeyDown = (e: React.KeyboardEvent<HTMLDivElement>) => {
    if (!page || e.key !== 'Escape' || e.defaultPrevented) return;
    if (!escapeGoesBack(e.target as HTMLElement)) return;
    e.preventDefault();
    back();
  };

  const slackTab = (page?.connector === 'slack' ? page.tab : null) as SlackTabId | null;
  const telegramTab = (page?.connector === 'telegram' ? page.tab : null) as TelegramTabId | null;

  return (
    <div ref={rootRef} onKeyDown={onKeyDown} className="h-full overflow-y-auto scrollbar-thin bg-shodh-ground text-shodh-text">
      <div className="max-w-5xl mx-auto px-4 sm:px-8 py-7 flex flex-col gap-6">
        {page?.connector === 'slack' ? (
          <SlackPage
            slack={slack}
            wizard={wizard}
            onWizard={setWizard}
            tab={slackTab ?? 'overview'}
            onTab={next => show({ connector: 'slack', tab: next })}
            onBack={back}
          />
        ) : page?.connector === 'telegram' ? (
          <TelegramPage
            telegram={telegram}
            wizard={telegramWizard}
            onWizard={setTelegramWizard}
            tab={telegramTab ?? 'overview'}
            onTab={next => show({ connector: 'telegram', tab: next })}
            onBack={back}
          />
        ) : (
          <>
            <header className="flex flex-col gap-1">
              <h2 className="m-0 text-[20px] font-semibold text-shodh-text">Connectors</h2>
              <p className="m-0 text-[13px] text-shodh-text-secondary max-w-[640px]">
                Tools Shodh works in. Nothing connects until you set it up, and each one says what it can reach.
              </p>
            </header>
            <ul ref={tilesRef} aria-label="Connectors" className="m-0 p-0 list-none grid grid-cols-1 md:grid-cols-2 xl:grid-cols-3 gap-3">
              {CONNECTORS.map(info => {
                const open = () => show({ connector: info.id, tab: null });
                if (info.id === 'telegram') {
                  const status = telegram.status;
                  return (
                    <ConnectorTile
                      key={info.id}
                      info={info}
                      mark={<TelegramMark size="md" />}
                      pill={status ? telegramPill(status) : null}
                      action={status ? telegramTileAction(status) : 'Open'}
                      loadError={telegram.loadError}
                      onOpen={open}
                    />
                  );
                }
                const status = slack.status;
                return (
                  <ConnectorTile
                    key={info.id}
                    info={info}
                    mark={<SlackMark size="md" />}
                    pill={status ? slackPill(status) : null}
                    action={status ? tileAction(status) : 'Open'}
                    loadError={slack.loadError}
                    onOpen={open}
                  />
                );
              })}
            </ul>
          </>
        )}
      </div>
    </div>
  );
}
