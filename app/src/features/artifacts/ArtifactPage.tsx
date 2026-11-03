import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  ArrowLeft,
  Check,
  Download,
  FileText,
  Folder,
  FolderOpen,
  Globe,
  Loader2,
  MessageCircle,
  Pencil,
  RefreshCw,
  Trash2,
} from 'lucide-react';
import { cn } from '../../lib/utils';
import { notify } from '../../lib/notify';
import { relativeTime } from '../../utils/time';
import { showInFolder } from '../library/fileActions';
import { useChatSession } from '../ask/ChatSessionContext';
import { isLive } from '../agent/reducer';
import { creationsApi, errorText, onCreationsChanged } from './api';
import {
  ARTIFACT_PANES,
  PANE_LABELS,
  PREVIEW_ALLOW,
  PREVIEW_NETWORK_OFF_TEXT,
  PREVIEW_NETWORK_ON_TEXT,
  PREVIEW_SANDBOX,
  STATUS_LABELS,
  buildProgress,
  fileRows,
  previousVersion,
  progressLine,
  sizeLabel,
} from './model';
import type { ArtifactPane, Creation, FileChange, FileContent, FileEntry } from './model';
import type { ArtifactSelection } from './ArtifactsView';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-shodh-surface';
const QUIET_BUTTON = cn(
  'h-8 px-2.5 inline-flex items-center gap-1.5 rounded-lg text-small text-shodh-text-secondary hover:bg-shodh-raised hover:text-shodh-text transition-colors duration-micro disabled:opacity-40',
  FOCUS_RING,
);
const OUTLINE_BUTTON = cn(
  'h-8 px-3 inline-flex items-center gap-1.5 rounded-lg border border-shodh-border text-small font-medium text-shodh-text hover:bg-shodh-raised transition-colors duration-micro disabled:opacity-40',
  FOCUS_RING,
);
const DANGER_BUTTON = cn(
  'h-8 px-3 inline-flex items-center gap-1.5 rounded-lg bg-shodh-error text-white text-small font-semibold hover:opacity-90 transition-opacity duration-micro disabled:opacity-40',
  FOCUS_RING,
);

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
 * One creation: its live preview (agent-built code in a sandboxed frame with no access
 * to the app, its files or the network unless the user allows it), its files, its saved
 * versions with what changed, and Keep · Change something · Delete it, Open folder and
 * Export.
 */
export function ArtifactPage({ selection, onBack, onOpenChat, onChange, onDeleted }: ArtifactPageProps) {
  const [creation, setCreation] = useState<Creation | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [pane, setPane] = useState<ArtifactPane>(selection.pane);
  const [version, setVersion] = useState<number | null>(selection.version);
  const [network, setNetwork] = useState(false);
  const [previewUrl, setPreviewUrl] = useState<string | null>(null);
  const [previewError, setPreviewError] = useState<string | null>(null);
  const [reloadSeq, setReloadSeq] = useState(0);
  const [files, setFiles] = useState<FileEntry[] | null>(null);
  const [openFile, setOpenFile] = useState<FileContent | null>(null);
  const [changes, setChanges] = useState<FileChange[] | null>(null);
  const [confirmingDelete, setConfirmingDelete] = useState(false);
  const [busy, setBusy] = useState<null | 'keep' | 'export' | 'delete'>(null);
  const id = selection.id;

  // A later request (the assistant opening another version or pane) applies at once.
  useEffect(() => {
    setPane(selection.pane);
    setVersion(selection.version);
  }, [selection.seq, selection.pane, selection.version]);

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
  const doneChanges = steps.filter(s => (s.tool === 'write' || s.tool === 'edit' || s.tool === 'run_command') && s.status === 'done').length;

  // The preview: a token-scoped URL of this version (or of the folder now).
  useEffect(() => {
    let cancelled = false;
    setPreviewError(null);
    creationsApi
      .preview(id, version, network)
      .then(info => {
        if (!cancelled) setPreviewUrl(info.url);
      })
      .catch(e => {
        if (!cancelled) setPreviewError(errorText(e));
      });
    return () => {
      cancelled = true;
    };
  }, [id, version, network]);

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
              <Loader2 className="w-3.5 h-3.5 animate-spin" aria-hidden="true" />
              Opening
            </p>
          )}
        </div>
      </div>
    );
  }

  const hasPage = (files ?? []).some(f => f.path === 'index.html');
  const versionLabel = version === null ? 'Now' : `Version ${version}`;

  return (
    <div className="h-full overflow-y-auto scrollbar-thin">
      <div className="max-w-6xl mx-auto px-8 py-6 flex flex-col gap-4">
        <button type="button" onClick={onBack} className={cn(QUIET_BUTTON, 'self-start -ml-2.5')}>
          <ArrowLeft className="w-3.5 h-3.5" aria-hidden="true" />
          All artifacts
        </button>

        <header className="flex items-start justify-between gap-4 flex-wrap">
          <div className="flex flex-col gap-1 min-w-0">
            <h1 className="m-0 text-xl font-bold text-shodh-text truncate">{creation.title}</h1>
            <p className="m-0 text-small text-shodh-text-muted">
              {STATUS_LABELS[creation.status]} · {creation.versions.length} {creation.versions.length === 1 ? 'version' : 'versions'} ·
              changed {relativeTime(creation.updatedAt)}
            </p>
          </div>
          <div className="flex items-center gap-1.5 flex-wrap">
            <button type="button" onClick={() => onOpenChat(creation.conversationId)} className={QUIET_BUTTON}>
              <MessageCircle className="w-3.5 h-3.5" aria-hidden="true" />
              Open chat
            </button>
            <button type="button" onClick={() => void showInFolder(creation.folder)} className={QUIET_BUTTON}>
              <FolderOpen className="w-3.5 h-3.5" aria-hidden="true" />
              Open folder
            </button>
            <button
              type="button"
              onClick={() => void exportZip()}
              disabled={busy !== null || creation.versions.length === 0}
              title={creation.versions.length === 0 ? 'A version is saved when the answer ends' : `Export ${versionLabel === 'Now' ? 'the latest version' : versionLabel.toLowerCase()} as a zip file in Documents/Shodh exports`}
              className={QUIET_BUTTON}
            >
              <Download className="w-3.5 h-3.5" aria-hidden="true" />
              Export
            </button>
          </div>
        </header>

        {(creation.summary || creation.choices.length > 0) && (
          <FinishSummary summary={creation.summary} choices={creation.choices} />
        )}

        <div className="flex items-center gap-2 flex-wrap">
          <button
            type="button"
            onClick={() => void keep()}
            disabled={busy !== null || creation.status === 'kept' || running}
            className={OUTLINE_BUTTON}
          >
            <Check className="w-3.5 h-3.5" aria-hidden="true" />
            {creation.status === 'kept' ? 'Kept' : 'Keep'}
          </button>
          <button type="button" onClick={() => onChange(creation.conversationId)} className={OUTLINE_BUTTON}>
            <Pencil className="w-3.5 h-3.5" aria-hidden="true" />
            Change something
          </button>
          <button
            type="button"
            onClick={() => setConfirmingDelete(true)}
            disabled={busy !== null || running}
            title={running ? 'Stop the answer first: it is still building' : undefined}
            className={OUTLINE_BUTTON}
          >
            <Trash2 className="w-3.5 h-3.5" aria-hidden="true" />
            Delete it
          </button>
        </div>
        {confirmingDelete && (
          <div role="alertdialog" aria-label="Delete this creation" className="flex flex-col gap-2 rounded-xl border border-shodh-error/40 bg-shodh-error/5 px-4 py-3">
            <p className="m-0 text-small text-shodh-text">
              Delete {creation.title}? Its folder ({creation.folder}) and every saved version are removed. This cannot be undone.
            </p>
            <div className="flex items-center gap-2">
              <button type="button" onClick={() => void remove()} disabled={busy !== null} className={DANGER_BUTTON}>
                <Trash2 className="w-3.5 h-3.5" aria-hidden="true" />
                Delete it
              </button>
              <button type="button" onClick={() => setConfirmingDelete(false)} disabled={busy === 'delete'} className={QUIET_BUTTON}>
                Cancel
              </button>
            </div>
          </div>
        )}

        {live && (
          <p role="status" aria-live="polite" className="m-0 inline-flex items-center gap-2 text-small text-shodh-text-secondary">
            {running && <Loader2 className="w-3.5 h-3.5 animate-spin" aria-hidden="true" />}
            {running ? progressLine(progress, running) : `Finished · ${progressLine(progress, false)}`}
          </p>
        )}

        <div className="flex items-center justify-between gap-3 flex-wrap border-b border-shodh-border-subtle">
          <div role="tablist" aria-label="Artifact" className="flex items-center gap-1">
            {ARTIFACT_PANES.map(name => (
              <button
                key={name}
                type="button"
                role="tab"
                aria-selected={pane === name}
                onClick={() => setPane(name)}
                className={cn(
                  'h-9 px-3 text-small font-medium border-b-2 -mb-px transition-colors duration-micro',
                  pane === name ? 'border-shodh-accent text-shodh-text' : 'border-transparent text-shodh-text-muted hover:text-shodh-text',
                  FOCUS_RING,
                )}
              >
                {PANE_LABELS[name]}
              </button>
            ))}
          </div>
          <label className="inline-flex items-center gap-2 text-small text-shodh-text-secondary pb-1.5">
            Showing
            <select
              value={version === null ? 'now' : String(version)}
              onChange={e => setVersion(e.target.value === 'now' ? null : Number(e.target.value))}
              className={cn('h-8 px-2 rounded-lg border border-shodh-border bg-shodh-surface text-small text-shodh-text', FOCUS_RING)}
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

        {pane === 'preview' && (
          <div className="flex flex-col gap-2">
            <div className="flex items-center justify-between gap-3 flex-wrap">
              <label className="inline-flex items-center gap-2 text-small text-shodh-text-secondary">
                <input
                  type="checkbox"
                  checked={network}
                  onChange={e => setNetwork(e.target.checked)}
                  className={cn('accent-shodh-accent', FOCUS_RING)}
                />
                <Globe className="w-3.5 h-3.5" aria-hidden="true" />
                Let this preview reach the internet
              </label>
              <button type="button" onClick={() => setReloadSeq(seq => seq + 1)} className={QUIET_BUTTON}>
                <RefreshCw className="w-3.5 h-3.5" aria-hidden="true" />
                Reload
              </button>
            </div>
            <p className="m-0 text-caption text-shodh-text-muted">
              {network ? PREVIEW_NETWORK_ON_TEXT : PREVIEW_NETWORK_OFF_TEXT}
            </p>
            {previewError ? (
              <p role="alert" className="m-0 text-small text-shodh-error">The preview could not open: {previewError}</p>
            ) : !hasPage ? (
              <div className="flex items-center justify-center h-[420px] rounded-xl border border-dashed border-shodh-border text-small text-shodh-text-muted px-6 text-center">
                {files === null
                  ? 'Looking for the page'
                  : running
                    ? 'The preview appears once the build writes index.html.'
                    : 'There is no index.html at the top of this folder, so there is no page to show. Its files are in Files.'}
              </div>
            ) : previewUrl ? (
              <iframe
                key={`${previewUrl}#${reloadSeq}`}
                title={`Preview of ${creation.title}`}
                src={previewUrl}
                sandbox={PREVIEW_SANDBOX}
                allow={PREVIEW_ALLOW}
                referrerPolicy="no-referrer"
                className="w-full h-[min(70vh,720px)] rounded-xl border border-shodh-border bg-white"
              />
            ) : null}
          </div>
        )}

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
                          'w-full flex items-center gap-1.5 h-7 pr-3 text-left text-small rounded-md hover:bg-shodh-raised',
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
                  {openFile.binary ? (
                    <p className="m-0 px-3 py-3 text-small text-shodh-text-muted">This file is not text, so it is not shown here. Open the folder to see it.</p>
                  ) : (
                    <pre className="m-0 p-3 text-caption leading-relaxed text-shodh-text overflow-auto max-h-[64vh] whitespace-pre font-mono">
                      {openFile.text}
                    </pre>
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
                        'w-full text-left flex flex-col gap-0.5 rounded-lg border px-3 py-2',
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
                changes.map(change => <ChangeView key={change.path} change={change} />)
              )}
            </div>
          </div>
        )}
      </div>
    </div>
  );
}

/** "Here's what I chose": the agent's summary and its choices, as plain text. */
export function FinishSummary({ summary, choices }: { summary: string | null; choices: string[] }) {
  return (
    <section aria-label="Here's what I chose" className="flex flex-col gap-1.5 rounded-xl border border-shodh-border-subtle bg-shodh-surface px-4 py-3">
      {summary && <p className="m-0 text-small text-shodh-text">{summary}</p>}
      {choices.length > 0 && (
        <>
          <p className="m-0 text-caption font-semibold text-shodh-text-secondary">Here&apos;s what I chose</p>
          <ul className="m-0 pl-5 flex flex-col gap-0.5 text-small text-shodh-text-secondary list-disc">
            {choices.map(choice => (
              <li key={choice}>{choice}</li>
            ))}
          </ul>
        </>
      )}
    </section>
  );
}

const STATUS_WORDS: Record<FileChange['status'], string> = {
  added: 'Added',
  removed: 'Removed',
  changed: 'Changed',
};

/** One file's change between two versions. */
function ChangeView({ change }: { change: FileChange }) {
  return (
    <section className="rounded-xl border border-shodh-border-subtle overflow-hidden">
      <p className="m-0 px-3 py-2 text-caption text-shodh-text-secondary border-b border-shodh-border-subtle">
        <span className="font-semibold">{STATUS_WORDS[change.status]}</span> · {change.path}
      </p>
      {change.binary ? (
        <p className="m-0 px-3 py-2 text-small text-shodh-text-muted">Not text: only that it changed is shown.</p>
      ) : change.tooLarge ? (
        <p className="m-0 px-3 py-2 text-small text-shodh-text-muted">Too large to compare line by line.</p>
      ) : (
        <div className="overflow-x-auto">
          {change.hunks.map((hunk, index) => (
            <pre key={index} className="m-0 py-1 text-caption leading-relaxed font-mono border-b border-shodh-border-subtle last:border-b-0">
              {hunk.lines.map((line, at) => (
                <div
                  key={at}
                  className={cn(
                    'px-3 whitespace-pre',
                    line.kind === 'added' && 'bg-shodh-success/10 text-shodh-text',
                    line.kind === 'removed' && 'bg-shodh-error/10 text-shodh-text',
                    line.kind === 'same' && 'text-shodh-text-muted',
                  )}
                >
                  <span className="select-none inline-block w-4 text-shodh-text-faint" aria-hidden="true">
                    {line.kind === 'added' ? '+' : line.kind === 'removed' ? '−' : ' '}
                  </span>
                  <span className="sr-only">{line.kind === 'added' ? 'Added: ' : line.kind === 'removed' ? 'Removed: ' : ''}</span>
                  {line.text}
                </div>
              ))}
            </pre>
          ))}
        </div>
      )}
    </section>
  );
}

export default ArtifactPage;
