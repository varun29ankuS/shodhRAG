import React, { useCallback, useEffect, useId, useMemo, useRef, useState } from 'react';
import {
  ArrowLeft,
  Check,
  CheckCircle2,
  ChevronRight,
  Download,
  FileText,
  Folder,
  FolderOpen,
  Globe,
  Loader2,
  MessageCircle,
  MoreHorizontal,
  Palette,
  Pencil,
  Trash2,
} from 'lucide-react';
import type { LucideIcon } from 'lucide-react';
import type { UnlistenFn } from '@tauri-apps/api/event';
import { cn } from '../../lib/utils';
import { notify } from '../../lib/notify';
import { relativeTime } from '../../utils/time';
import { IS_MAC } from '../../components/ui/Kbd';
import { showInFolder } from '../library/fileActions';
import { useChatSession } from '../ask/ChatSessionContext';
import { isLive } from '../agent/reducer';
import { creationsApi, errorText, onCreationsChanged } from './api';
import { COMPACT_LABEL, EmbeddedPreview } from './EmbeddedPreview';
import {
  ARTIFACT_PANES,
  PANE_LABELS,
  STATUS_LABELS,
  buildProgress,
  fileRows,
  hasEntryPage,
  previewNetworkText,
  previousVersion,
  progressLine,
  sizeLabel,
} from './model';
import type {
  ArtifactPane,
  Creation,
  FileChange,
  FileContent,
  FileEntry,
} from './model';
import type { ArtifactSelection } from './ArtifactsView';
import { CodeDiff, CodeFileView } from '../code/CodeDiff';
import { languageOf } from '../code/diffModel';
import type { LineSelection } from '../code/diffModel';
import { ReviewThisButton } from '../code/ReviewThisButton';
import { useCodeReview } from '../code/CodeReviewContext';
import { designApi, onDesignCheckUpdated } from '../code/api';
import { badgeDetail, badgeText, totalIssues } from '../code/designModel';
import type { StoredDesignCheck } from '../code/designModel';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-shodh-surface';
const QUIET_BUTTON = cn(
  'h-8 px-2.5 shrink-0 inline-flex items-center gap-1.5 rounded-lg text-small text-shodh-text-secondary whitespace-nowrap hover:bg-shodh-raised hover:text-shodh-text transition-colors duration-micro disabled:opacity-40',
  FOCUS_RING,
);
const OUTLINE_BUTTON = cn(
  'h-8 px-3 shrink-0 inline-flex items-center gap-1.5 rounded-lg border border-shodh-border text-small font-medium text-shodh-text whitespace-nowrap hover:bg-shodh-raised transition-colors duration-micro disabled:opacity-40',
  FOCUS_RING,
);
const DANGER_BUTTON = cn(
  'h-8 px-3 shrink-0 inline-flex items-center gap-1.5 whitespace-nowrap rounded-lg bg-shodh-error text-white text-small font-semibold hover:opacity-90 transition-opacity duration-micro disabled:opacity-40',
  FOCUS_RING,
);

/**
 * Below this width (CSS pixels) the page's header and toolbar keep to icons for their
 * secondary actions, so each stays one row with every control on screen (a pane docked
 * beside the page narrows it).
 */
const COMPACT_BELOW = 860;

/** Reloads of the live preview while a build writes files, at most this often. */
const LIVE_RELOAD_MS = 1500;

interface ArtifactPageProps {
  selection: ArtifactSelection;
  onBack: () => void;
  onOpenChat: (conversationId: string) => void;
  onChange: (conversationId: string) => void;
  onDeleted: () => void;
}

/**
 * One creation: its live preview (agent-built code in a locked webview over the preview pane,
 * or popped out to a window of its own, with no access to the app, its files or the network unless the
 * user allows it), its files, its saved versions with what changed, and Keep · Change something ·
 * Delete it, Open folder and Export.
 */
export function ArtifactPage({ selection, onBack, onOpenChat, onChange, onDeleted }: ArtifactPageProps) {
  const [creation, setCreation] = useState<Creation | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [pane, setPane] = useState<ArtifactPane>(selection.pane);
  const [version, setVersion] = useState<number | null>(selection.version);
  const [network, setNetwork] = useState(false);
  const [reloadSeq, setReloadSeq] = useState(0);
  // Increases each time the preview window should open: the user clicks Preview, or the
  // request that opened this page asked for the preview (the assistant's open_artifact).
  const [openSeq, setOpenSeq] = useState(selection.showPreview ? 1 : 0);
  const [files, setFiles] = useState<FileEntry[] | null>(null);
  const [openFile, setOpenFile] = useState<FileContent | null>(null);
  const [changes, setChanges] = useState<FileChange[] | null>(null);
  const [confirmingDelete, setConfirmingDelete] = useState(false);
  const [busy, setBusy] = useState<null | 'keep' | 'export' | 'delete'>(null);
  const id = selection.id;
  const networkNoteId = useId();
  // The page's width, for the compact header and toolbar.
  const [root, setRoot] = useState<HTMLDivElement | null>(null);
  const [compact, setCompact] = useState(false);
  useEffect(() => {
    if (!root) return;
    const measure = () => setCompact(root.clientWidth < COMPACT_BELOW);
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(root);
    return () => observer.disconnect();
  }, [root]);

  // A later request (the assistant opening another version or pane) applies at once.
  const firstSeq = useRef(selection.seq);
  useEffect(() => {
    setPane(selection.pane);
    setVersion(selection.version);
    if (selection.seq !== firstSeq.current && selection.showPreview) setOpenSeq(seq => seq + 1);
  }, [selection.seq, selection.pane, selection.version, selection.showPreview]);

  const load = useCallback(() => {
    creationsApi
      .get(id)
      .then(found => {
        setCreation(found);
        setLoadError(null);
      })
      .catch(e => setLoadError(errorText(e)));
  }, [id]);

  useEffect(() => {
    load();
    let disposed = false;
    let unlisten: (() => void) | null = null;
    onCreationsChanged(change => {
      if (change.creationId === id) load();
    })
      .then(fn => {
        if (disposed) fn();
        else unlisten = fn;
      })
      .catch(e => console.error('Failed to listen for creations-changed:', e));
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [id, load]);

  // The build in progress, when this creation's conversation is answering now.
  const { liveRun } = useChatSession();
  const live = liveRun && creation && liveRun.conversationId === creation.conversationId ? liveRun.transcript : null;
  const running = live !== null && isLive(live);
  const steps = useMemo(
    () => (live ? Object.values(live.steps).sort((a, b) => a.startedAtMs - b.startedAtMs) : []),
    [live],
  );
  const progress = useMemo(() => buildProgress(steps), [steps]);
  // The design check of its files: a badge in the header, never a pane over the page.
  const design = useArtifactDesignCheck(id, creation?.conversationId ?? null);
  const doneChanges = steps.filter(s => (s.tool === 'write' || s.tool === 'edit' || s.tool === 'run_command') && s.status === 'done').length;

  // The files of the version shown (the folder now while building).
  const loadFiles = useCallback(() => {
    creationsApi
      .files(id, version)
      .then(setFiles)
      .catch(e => {
        setFiles([]);
        notify.error('The files could not be listed', { description: errorText(e) });
      });
  }, [id, version]);
  useEffect(() => {
    loadFiles();
    setOpenFile(null);
  }, [loadFiles]);

  // While it builds: the file list fills in and the preview reloads, at most every 1.5 s.
  const lastReload = useRef(0);
  useEffect(() => {
    if (version !== null || doneChanges === 0) return;
    loadFiles();
    const wait = Math.max(0, LIVE_RELOAD_MS - (Date.now() - lastReload.current));
    const timer = window.setTimeout(() => {
      lastReload.current = Date.now();
      setReloadSeq(seq => seq + 1);
    }, wait);
    return () => window.clearTimeout(timer);
  }, [doneChanges, version, loadFiles]);

  // What changed in the version shown, against the one before it.
  const shownVersion = version ?? creation?.versions[creation.versions.length - 1]?.number ?? null;
  useEffect(() => {
    if (pane !== 'versions' || !creation || shownVersion === null) {
      setChanges(null);
      return;
    }
    let cancelled = false;
    creationsApi
      .diff(id, previousVersion(creation.versions, shownVersion), shownVersion)
      .then(found => {
        if (!cancelled) setChanges(found);
      })
      .catch(e => {
        if (!cancelled) {
          setChanges([]);
          notify.error('The versions could not be compared', { description: errorText(e) });
        }
      });
    return () => {
      cancelled = true;
    };
  }, [pane, creation, shownVersion, id]);

  const showFile = (path: string) => {
    creationsApi
      .file(id, version, path)
      .then(setOpenFile)
      .catch(e => notify.error('The file could not be opened', { description: errorText(e) }));
  };

  const keep = async () => {
    setBusy('keep');
    try {
      setCreation(await creationsApi.keep(id));
      notify.success('Kept', { description: 'It stays in Artifacts and in its folder.' });
    } catch (e) {
      notify.error('It could not be kept', { description: errorText(e) });
    } finally {
      setBusy(null);
    }
  };

  const exportZip = async () => {
    setBusy('export');
    try {
      const path = await creationsApi.exportZip(id, version);
      notify.success('Exported', { description: path });
    } catch (e) {
      notify.error('It could not be exported', { description: errorText(e) });
    } finally {
      setBusy(null);
    }
  };

  const remove = async () => {
    setBusy('delete');
    try {
      await creationsApi.remove(id);
      notify.success('Deleted', { description: 'The folder and every saved version are gone.' });
      onDeleted();
    } catch (e) {
      notify.error('It could not be deleted', { description: errorText(e) });
      setBusy(null);
    }
  };

  if (!creation) {
    return (
      <div className="h-full overflow-y-auto">
        <div className="max-w-5xl mx-auto px-8 py-7 flex flex-col gap-4">
          <button type="button" onClick={onBack} className={cn(QUIET_BUTTON, 'self-start')}>
            <ArrowLeft className="w-3.5 h-3.5" aria-hidden="true" />
            All artifacts
          </button>
          {loadError ? (
            <p role="alert" className="m-0 text-small text-shodh-error">{loadError}</p>
          ) : (
            <p role="status" className="m-0 inline-flex items-center gap-2 text-small text-shodh-text-muted">
              <Loader2 className="w-3.5 h-3.5 animate-spin motion-reduce:animate-none" aria-hidden="true" />
              Opening
            </p>
          )}
        </div>
      </div>
    );
  }

  const hasPage = hasEntryPage(files);
  const versionLabel = version === null ? 'Now' : `Version ${version}`;
  const statusText = live
    ? running ? progressLine(progress, running) : `Finished · ${progressLine(progress, false)}`
    : `${STATUS_LABELS[creation.status]} · ${creation.versions.length} ${creation.versions.length === 1 ? 'version' : 'versions'} · changed ${relativeTime(creation.updatedAt)}`;

  // The panes and the version shown: the start of the toolbar row, beside the page's own controls.
  const panesAndVersion = (
    <div className="flex items-center gap-2 shrink-0">
      <div role="tablist" aria-label="Artifact" className="flex items-center gap-0.5 rounded-lg bg-shodh-raised p-0.5">
        {ARTIFACT_PANES.map(name => (
          <button
            key={name}
            type="button"
            role="tab"
            aria-selected={pane === name}
            onClick={() => {
              setPane(name);
              // Clicking Preview shows the page (a popped-out window comes to the front).
              if (name === 'preview') setOpenSeq(seq => seq + 1);
            }}
            className={cn(
              'h-7 px-2.5 rounded-md text-small font-medium whitespace-nowrap transition-colors duration-micro',
              pane === name ? 'bg-shodh-surface text-shodh-text shadow-sm' : 'text-shodh-text-muted hover:text-shodh-text',
              FOCUS_RING,
            )}
          >
            {PANE_LABELS[name]}
          </button>
        ))}
      </div>
      <label className="inline-flex items-center gap-1.5 text-small text-shodh-text-secondary whitespace-nowrap">
        <span className="sr-only">Showing</span>
        <select
          value={version === null ? 'now' : String(version)}
          onChange={e => setVersion(e.target.value === 'now' ? null : Number(e.target.value))}
          className={cn('h-8 max-w-[220px] group-data-[compact=true]/artifact:max-w-[120px] px-2 rounded-lg border border-shodh-border bg-shodh-surface text-small text-shodh-text', FOCUS_RING)}
        >
          <option value="now">The folder now</option>
          {[...creation.versions].reverse().map(v => (
            <option key={v.number} value={v.number}>
              Version {v.number} · {relativeTime(v.createdAt)}
            </option>
          ))}
        </select>
      </label>
    </div>
  );

  // The internet switch, compact: what it means is its tooltip and its description.
  const networkText = previewNetworkText(network, IS_MAC);
  const networkControl = (
    <label title={networkText} className="h-8 px-2 shrink-0 inline-flex items-center gap-1.5 rounded-lg text-small text-shodh-text-secondary whitespace-nowrap hover:bg-shodh-raised">
      <input
        type="checkbox"
        checked={network}
        onChange={e => setNetwork(e.target.checked)}
        aria-describedby={networkNoteId}
        className={cn('accent-shodh-accent', FOCUS_RING)}
      />
      <Globe className="w-3.5 h-3.5" aria-hidden="true" />
      <span className={COMPACT_LABEL}>Internet</span>
      <span id={networkNoteId} className="sr-only">{networkText}</span>
    </label>
  );

  return (
    <div ref={setRoot} data-compact={compact} className="group/artifact h-full flex flex-col min-h-0">
      <div className="flex-1 min-h-0 flex flex-col gap-2 px-5 pt-3 pb-3">
        <header className="flex items-center gap-2 flex-nowrap min-w-0">
          <button type="button" onClick={onBack} aria-label="All artifacts" title="All artifacts" className={cn(QUIET_BUTTON, 'px-2 -ms-2')}>
            <ArrowLeft className="w-4 h-4" aria-hidden="true" />
          </button>
          <h1 className="m-0 min-w-[6rem] shrink truncate text-subhead font-bold text-shodh-text" title={creation.title}>{creation.title}</h1>
          <p role="status" aria-live="polite" className="m-0 min-w-0 truncate inline-flex items-center gap-1.5 text-small text-shodh-text-muted" title={statusText}>
            {running && <Loader2 className="w-3.5 h-3.5 shrink-0 animate-spin motion-reduce:animate-none" aria-hidden="true" />}
            <span className="truncate">{statusText}</span>
          </p>
          <div className="ms-auto flex items-center gap-1.5 shrink-0">
            <DesignBadge design={design} />
            <button
              type="button"
              onClick={() => void keep()}
              disabled={busy !== null || creation.status === 'kept' || running}
              className={OUTLINE_BUTTON}
            >
              <Check className="w-3.5 h-3.5" aria-hidden="true" />
              {creation.status === 'kept' ? 'Kept' : 'Keep'}
            </button>
            <button type="button" onClick={() => onChange(creation.conversationId)} title="Change something: the chat that built it, with your request started" className={OUTLINE_BUTTON}>
              <Pencil className="w-3.5 h-3.5" aria-hidden="true" />
              <span className={COMPACT_LABEL}>Change</span>
            </button>
            <ArtifactActionsMenu
              items={[
                { label: 'Open chat', icon: MessageCircle, onSelect: () => onOpenChat(creation.conversationId) },
                { label: 'Open folder', icon: FolderOpen, onSelect: () => void showInFolder(creation.folder) },
                {
                  label: 'Export',
                  icon: Download,
                  onSelect: () => void exportZip(),
                  disabled: busy !== null || creation.versions.length === 0,
                  title: creation.versions.length === 0 ? 'A version is saved when the answer ends' : `Export ${versionLabel === 'Now' ? 'the latest version' : versionLabel.toLowerCase()} as a zip file in Documents/Shodh exports`,
                },
                {
                  label: design.check ? 'Check the design again' : 'Design check',
                  icon: Palette,
                  onSelect: () => void design.run(),
                  disabled: design.busy,
                  title: "Check this artifact's HTML, CSS and components with Shodh's design rules (no model): contrast, text size, labels, alt text, focus, generated-UI tells.",
                },
                {
                  label: 'Delete it',
                  icon: Trash2,
                  onSelect: () => setConfirmingDelete(true),
                  disabled: busy !== null || running,
                  title: running ? 'Stop the answer first: it is still building' : undefined,
                },
              ]}
            />
          </div>
        </header>

        {confirmingDelete && (
          <div role="alertdialog" aria-label="Delete this creation" className="flex items-center gap-3 flex-nowrap rounded-xl border border-shodh-error/40 bg-shodh-error/5 px-4 py-2">
            <p className="m-0 min-w-0 flex-1 text-small text-shodh-text">
              Delete {creation.title}? Its folder ({creation.folder}) and every saved version are removed. This cannot be undone.
            </p>
            <button type="button" onClick={() => void remove()} disabled={busy !== null} className={DANGER_BUTTON}>
              <Trash2 className="w-3.5 h-3.5" aria-hidden="true" />
              Delete it
            </button>
            <button type="button" onClick={() => setConfirmingDelete(false)} disabled={busy === 'delete'} className={QUIET_BUTTON}>
              Cancel
            </button>
          </div>
        )}

        {(creation.summary || creation.choices.length > 0) && (
          <FinishSummary summary={creation.summary} choices={creation.choices} />
        )}

        {pane === 'preview' && (
          !hasPage ? (
            <div className="flex-1 min-h-0 flex flex-col gap-2">
              <div className="flex items-center gap-2 flex-nowrap min-w-0">
                {panesAndVersion}
                <div className="ms-auto">{networkControl}</div>
              </div>
              <div className="flex-1 min-h-0 flex items-center justify-center rounded-xl border border-dashed border-shodh-border text-small text-shodh-text-muted px-6 text-center">
                {files === null
                  ? 'Looking for the page'
                  : running
                    ? 'The preview appears once the build writes index.html.'
                    : 'There is no index.html at the top of this folder, so there is no page to show. Its files are in Files.'}
              </div>
            </div>
          ) : (
            <EmbeddedPreview
              creationId={id}
              version={version}
              network={network}
              reloadSeq={reloadSeq}
              focusSeq={openSeq}
              leading={panesAndVersion}
              networkControl={networkControl}
            />
          )
        )}

        {pane !== 'preview' && (
          <div className="flex items-center gap-2 flex-nowrap min-w-0">{panesAndVersion}</div>
        )}
        {pane !== 'preview' && (
          <div className="flex-1 min-h-0 overflow-y-auto scrollbar-thin flex flex-col gap-4">
        {pane === 'files' && (
          <div className="grid gap-4 grid-cols-[minmax(220px,300px)_1fr] min-h-[360px]">
            <ul aria-label="Files" className="list-none m-0 p-0 flex flex-col gap-0.5 overflow-y-auto max-h-[70vh] rounded-xl border border-shodh-border-subtle py-2">
              {files === null ? (
                <li className="px-3 text-small text-shodh-text-muted">Listing</li>
              ) : files.length === 0 ? (
                <li className="px-3 text-small text-shodh-text-muted">No files yet.</li>
              ) : (
                fileRows(files).map(row => (
                  <li key={row.key}>
                    {row.folder ? (
                      <span
                        className="flex items-center gap-1.5 h-7 text-small text-shodh-text-secondary"
                        style={{ paddingLeft: 12 + row.depth * 14 }}
                      >
                        <Folder className="w-3.5 h-3.5 shrink-0" aria-hidden="true" />
                        {row.name}
                      </span>
                    ) : (
                      <button
                        type="button"
                        onClick={() => showFile(row.key)}
                        aria-current={openFile?.path === row.key}
                        className={cn(
                          'w-full flex items-center gap-1.5 h-7 pe-3 text-start text-small rounded-md hover:bg-shodh-raised',
                          openFile?.path === row.key ? 'bg-shodh-raised text-shodh-text' : 'text-shodh-text-secondary',
                          FOCUS_RING,
                        )}
                        style={{ paddingLeft: 12 + row.depth * 14 }}
                      >
                        <FileText className="w-3.5 h-3.5 shrink-0" aria-hidden="true" />
                        <span className="truncate flex-1">{row.name}</span>
                        <span className="text-caption text-shodh-text-faint">{sizeLabel(row.size)}</span>
                      </button>
                    )}
                  </li>
                ))
              )}
            </ul>
            <div className="min-w-0 rounded-xl border border-shodh-border-subtle overflow-hidden flex flex-col">
              {openFile ? (
                <>
                  <p className="m-0 px-3 py-2 text-caption text-shodh-text-muted border-b border-shodh-border-subtle">
                    {openFile.path} · {sizeLabel(openFile.size)}
                    {openFile.truncated ? ' · only the start is shown' : ''}
                  </p>
                  {openFile.binary || openFile.text === null ? (
                    <p className="m-0 px-3 py-3 text-small text-shodh-text-muted">This file is not text, so it is not shown here. Open the folder to see it.</p>
                  ) : (
                    <ArtifactFile path={openFile.path} text={openFile.text} title={creation.title} />
                  )}
                </>
              ) : (
                <p className="m-0 px-3 py-3 text-small text-shodh-text-muted">Pick a file to see it.</p>
              )}
            </div>
          </div>
        )}

        {pane === 'versions' && (
          <div className="grid gap-4 grid-cols-[minmax(220px,280px)_1fr]">
            <ul aria-label="Versions" className="list-none m-0 p-0 flex flex-col gap-1">
              {creation.versions.length === 0 ? (
                <li className="text-small text-shodh-text-muted">No version yet: one is saved when an answer ends.</li>
              ) : (
                [...creation.versions].reverse().map(v => (
                  <li key={v.number}>
                    <button
                      type="button"
                      onClick={() => setVersion(v.number)}
                      aria-current={shownVersion === v.number}
                      className={cn(
                        'w-full text-start flex flex-col gap-0.5 rounded-lg border px-3 py-2',
                        shownVersion === v.number ? 'border-shodh-border-strong bg-shodh-raised' : 'border-transparent hover:bg-shodh-raised',
                        FOCUS_RING,
                      )}
                    >
                      <span className="text-small font-medium text-shodh-text">Version {v.number}</span>
                      <span className="text-caption text-shodh-text-muted">
                        {relativeTime(v.createdAt)} · {v.files} {v.files === 1 ? 'file' : 'files'} · {sizeLabel(v.bytes)}
                        {v.leftOut > 0 ? ` · ${v.leftOut} left out` : ''}
                      </span>
                    </button>
                  </li>
                ))
              )}
            </ul>
            <div className="min-w-0 flex flex-col gap-3">
              {shownVersion !== null && (
                <p className="m-0 text-small text-shodh-text-secondary">
                  {previousVersion(creation.versions, shownVersion) === 0
                    ? `Version ${shownVersion}: everything it first made.`
                    : `Version ${shownVersion} against version ${previousVersion(creation.versions, shownVersion)}.`}
                </p>
              )}
              {changes === null ? null : changes.length === 0 ? (
                <p className="m-0 text-small text-shodh-text-muted">Nothing changed.</p>
              ) : (
                <VersionChanges changes={changes} title={`${creation.title}, version ${shownVersion}`} />
              )}
            </div>
          </div>
        )}
          </div>
        )}
      </div>
    </div>
  );
}

/**
 * "Here's what I chose": the agent's summary and its choices, as plain text, folded to one line
 * under the header so the page keeps the height. Opening it shows the choices.
 */
export function FinishSummary({ summary, choices }: { summary: string | null; choices: string[] }) {
  return (
    <details aria-label="Here's what I chose" className="group shrink-0 rounded-lg border border-shodh-border-subtle bg-shodh-surface">
      <summary className={cn('flex items-center gap-2 px-3 h-8 cursor-pointer list-none text-small text-shodh-text-secondary [&::-webkit-details-marker]:hidden', FOCUS_RING)}>
        <ChevronRight className="w-3.5 h-3.5 shrink-0 transition-transform duration-micro group-open:rotate-90" aria-hidden="true" />
        <span className="min-w-0 truncate">
          {summary ?? 'Here’s what I chose'}
        </span>
        {choices.length > 0 && (
          <span className="ms-auto shrink-0 text-caption text-shodh-text-muted whitespace-nowrap">
            {choices.length} {choices.length === 1 ? 'choice' : 'choices'}
          </span>
        )}
      </summary>
      <div className="flex flex-col gap-1.5 px-4 pb-3 pt-1 max-h-[40vh] overflow-y-auto scrollbar-thin">
        {summary && <p className="m-0 text-small text-shodh-text">{summary}</p>}
        {choices.length > 0 && (
          <>
            <p className="m-0 text-caption font-semibold text-shodh-text-secondary">Here&apos;s what I chose</p>
            <ul className="m-0 ps-5 flex flex-col gap-0.5 text-small text-shodh-text-secondary list-disc">
              {choices.map(choice => (
                <li key={choice}>{choice}</li>
              ))}
            </ul>
          </>
        )}
      </div>
    </details>
  );
}

/** What changed in a version, in the app's one diff view, reviewable. */
function VersionChanges({ changes, title }: { changes: FileChange[]; title: string }) {
  const [selection, setSelection] = useState<LineSelection | null>(null);
  return (
    <CodeDiff
      changes={changes}
      label={`What changed in ${title}`}
      selection={selection}
      onSelect={setSelection}
      maxHeight={640}
      actions={<ReviewThisButton subject={() => ({ kind: 'diff', title, changes, workspaceId: null, selection })} />}
    />
  );
}

/** A file of a creation, with line numbers, reviewable. */
function ArtifactFile({ path, text, title }: { path: string; text: string; title: string }) {
  const [selection, setSelection] = useState<LineSelection | null>(null);
  const language = languageOf(path);
  return (
    <CodeFileView
      path={path}
      text={text}
      language={language}
      label={`${path} of ${title}`}
      selection={selection}
      onSelect={setSelection}
      maxHeight={640}
      actions={
        <ReviewThisButton
          subject={() => ({ kind: 'file', title: path, path, text, language, firstLine: 1, workspaceId: null, selection })}
        />
      }
    />
  );
}

/** The artifact's latest design check, kept up to date, and running one now. */
interface ArtifactDesign {
  check: StoredDesignCheck | null;
  busy: boolean;
  /** Check its files now, then show the findings (the pane docks beside the page). */
  run: () => Promise<void>;
  /** Show the latest check's findings. */
  open: () => void;
}

/**
 * The design check of a creation's files. Shodh also checks them after each round of writes
 * while it builds, and the assistant may run one: each new check updates the header's badge
 * and nothing else, so the live page is never covered. No model is used.
 */
function useArtifactDesignCheck(creationId: string, conversationId: string | null): ArtifactDesign {
  const pane = useCodeReview();
  const [check, setCheck] = useState<StoredDesignCheck | null>(null);
  const [busy, setBusy] = useState(false);
  const generation = useRef(0);

  const load = useCallback(() => {
    const mine = ++generation.current;
    designApi
      .latest(null, creationId)
      .then(found => {
        if (mine === generation.current) setCheck(found);
      })
      .catch(e => console.error('Design check unavailable:', String(e)));
  }, [creationId]);

  useEffect(() => {
    setCheck(null);
    load();
    let disposed = false;
    let unlisten: UnlistenFn | null = null;
    onDesignCheckUpdated(update => {
      if (update.creationId === creationId) load();
    })
      .then(fn => {
        if (disposed) fn();
        else unlisten = fn;
      })
      .catch(e => console.error('Failed to listen for design-check-updated:', e));
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [creationId, load]);

  const run = useCallback(async () => {
    if (!conversationId) return;
    setBusy(true);
    try {
      const found = await designApi.run({ creationId, conversationId });
      generation.current += 1;
      setCheck(found);
      pane?.openDesign(found);
    } catch (e) {
      notify.error('The design check did not run', { description: String(e) });
    } finally {
      setBusy(false);
    }
  }, [creationId, conversationId, pane]);

  const open = useCallback(() => {
    if (check) pane?.openDesign(check);
  }, [check, pane]);

  return { check, busy, run, open };
}

/**
 * The header's design badge ("Design check: 3 issues", "No issues"): one line, and the
 * findings open beside the page when clicked. Before any check, it runs one.
 */
function DesignBadge({ design }: { design: ArtifactDesign }) {
  const { check, busy } = design;
  const errors = check?.report.counts.error ?? 0;
  return (
    <button
      type="button"
      onClick={() => (check ? design.open() : void design.run())}
      disabled={busy}
      title={check ? `${badgeDetail(check)} Click to see the findings beside the page.` : "Check this artifact's HTML, CSS and components with Shodh's design rules (no model)."}
      className={cn(QUIET_BUTTON, 'whitespace-nowrap', errors > 0 && 'text-shodh-text')}
    >
      {busy
        ? <Loader2 className="w-3.5 h-3.5 animate-spin motion-reduce:animate-none" aria-hidden="true" />
        : check && totalIssues(check.report) === 0
          ? <CheckCircle2 className="w-3.5 h-3.5 text-shodh-success" aria-hidden="true" />
          : <Palette className="w-3.5 h-3.5" aria-hidden="true" />}
      <span className={COMPACT_LABEL}>{check ? badgeText(check) : 'Design check'}</span>
    </button>
  );
}

/** One action of the header's "More" menu. */
interface ActionItem {
  label: string;
  icon: LucideIcon;
  onSelect: () => void;
  disabled?: boolean;
  title?: string;
}

const MENU_ITEM =
  'w-full flex items-center gap-2 px-3 py-1.5 text-small text-start text-shodh-text-secondary whitespace-nowrap hover:bg-shodh-raised-2 focus-visible:bg-shodh-raised-2 focus-visible:outline-none disabled:opacity-40 disabled:hover:bg-transparent';

/** "⋯" at the header's end: the artifact's other actions, so the header stays one row. */
function ArtifactActionsMenu({ items }: { items: readonly ActionItem[] }) {
  const [open, setOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);
  const buttonRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const menuId = useId();

  useEffect(() => {
    if (!open) return;
    menuRef.current?.querySelector<HTMLButtonElement>('[role="menuitem"]:not(:disabled)')?.focus();
    const onPointer = (e: PointerEvent) => {
      if (rootRef.current && !rootRef.current.contains(e.target as Node)) setOpen(false);
    };
    document.addEventListener('pointerdown', onPointer);
    return () => document.removeEventListener('pointerdown', onPointer);
  }, [open]);

  const close = (restore: boolean) => {
    setOpen(false);
    if (restore) buttonRef.current?.focus();
  };

  const onMenuKeyDown = (e: React.KeyboardEvent<HTMLDivElement>) => {
    const buttons = Array.from(menuRef.current?.querySelectorAll<HTMLButtonElement>('[role="menuitem"]:not(:disabled)') ?? []);
    const index = buttons.indexOf(document.activeElement as HTMLButtonElement);
    if (e.key === 'Escape') {
      e.preventDefault();
      e.stopPropagation();
      close(true);
    } else if (e.key === 'ArrowDown') {
      e.preventDefault();
      buttons[(index + 1) % buttons.length]?.focus();
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      buttons[(index - 1 + buttons.length) % buttons.length]?.focus();
    } else if (e.key === 'Home' || e.key === 'End') {
      e.preventDefault();
      buttons[e.key === 'Home' ? 0 : buttons.length - 1]?.focus();
    } else if (e.key === 'Tab') {
      close(false);
    }
  };

  return (
    <div ref={rootRef} className="relative shrink-0">
      <button
        ref={buttonRef}
        type="button"
        onClick={() => setOpen(o => !o)}
        aria-label="More actions"
        aria-haspopup="menu"
        aria-expanded={open}
        aria-controls={open ? menuId : undefined}
        title="More actions"
        className={cn(QUIET_BUTTON, 'px-2')}
      >
        <MoreHorizontal className="w-4 h-4" aria-hidden="true" />
      </button>
      {open && (
        <div className="shell-pop absolute end-0 top-9 z-50 min-w-[220px] rounded-lg border border-shodh-border-strong bg-shodh-raised shadow-lg overflow-hidden">
          <div id={menuId} ref={menuRef} role="menu" aria-label="More actions" onKeyDown={onMenuKeyDown} className="py-1">
            {items.map(({ label, icon: Icon, onSelect, disabled, title }) => (
              <button
                key={label}
                type="button"
                role="menuitem"
                disabled={disabled}
                title={title}
                onClick={() => {
                  close(true);
                  onSelect();
                }}
                className={MENU_ITEM}
              >
                <Icon className="w-3.5 h-3.5" aria-hidden="true" />
                {label}
              </button>
            ))}
          </div>
        </div>
      )}
    </div>
  );
}

export default ArtifactPage;
