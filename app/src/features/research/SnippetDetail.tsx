import React, { useId, useMemo, useState } from 'react';
import katex from 'katex';
import 'katex/dist/katex.min.css';
import {
  AlertTriangle,
  Check,
  ClipboardCopy,
  FileSearch,
  ImageDown,
  Loader2,
  MessageSquarePlus,
  Pencil,
  Sigma,
  Table2,
  Trash2,
  X,
} from 'lucide-react';
import { cn } from '../../lib/utils';
import { notify } from '../../lib/notify';
import { removeWithUndo } from '../../lib/undoToast';
import { researchApi, toResearchError } from './api';
import { copyPngImage, copyText } from './snippetClipboard';
import { insertIntoComposer, showSourceBox } from './snippetBus';
import { KIND_LABEL, parseTags, snippetChatContext, snippetLabel, snippetSource } from './snippetModel';
import { pngDataUrl } from './snippetRender';
import type { Snippet, SnippetKind, SnippetTable } from './types';
import { SNIPPET_KINDS } from './types';
import { BUTTON, INPUT, PRIMARY_BUTTON, SECTION_TITLE } from './ui';
import { useSnippetImage, useVisionCapability } from './useSnippets';

/** LaTeX rendered by KaTeX, or why it could not be. */
function renderLatex(latex: string): { html: string } | { error: string } {
  try {
    return { html: katex.renderToString(latex, { displayMode: true, throwOnError: true, strict: 'ignore', trust: false }) };
  } catch (error) {
    return { error: error instanceof Error ? error.message : 'KaTeX could not parse it.' };
  }
}

function TableBlock({ table }: { table: SnippetTable }) {
  return (
    <div className="overflow-x-auto rounded-xl border border-shodh-border">
      <table className="min-w-full border-collapse text-[12.5px]">
        {table.caption && <caption className="caption-top px-3 py-2 text-left text-shodh-text-muted">{table.caption}</caption>}
        {table.header.length > 0 && (
          <thead className="bg-shodh-raised">
            <tr>
              {table.header.map((cell, i) => (
                <th key={i} scope="col" className="px-3 py-1.5 text-left font-semibold text-shodh-text border-b border-shodh-border">{cell}</th>
              ))}
            </tr>
          </thead>
        )}
        <tbody>
          {table.rows.map((row, r) => (
            <tr key={r}>
              {row.map((cell, c) => (
                <td key={c} className="px-3 py-1.5 align-top text-shodh-text-secondary border-b border-shodh-border-subtle tabular-nums">{cell}</td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

interface SnippetDetailProps {
  snippet: Snippet;
  /** In the focus pop-out ("Show in paper" and asking live in its header) or a standalone dialog. */
  mode: 'focus' | 'dialog';
  /** The snippet changed (edited, transcribed) or was deleted (null). */
  onChanged: (next: Snippet | null) => void;
  /** Open extracted rows as a table of their own (the pop-out drills down). */
  onOpenTable?: (rows: string[][]) => void;
}

/**
 * A snippet with its actions: the image drawn by pdf.js, the region's text,
 * its source, copy, add to chat, extract table, transcribe to LaTeX (with a
 * vision model), edit and delete.
 */
export function SnippetDetail({ snippet, mode, onChanged, onOpenTable }: SnippetDetailProps) {
  const image = useSnippetImage(snippet);
  const vision = useVisionCapability();
  const visionReasonId = useId();
  const titleId = useId();
  const noteId = useId();
  const tagsId = useId();
  const kindId = useId();
  const [editing, setEditing] = useState(false);
  const [title, setTitle] = useState(snippet.title);
  const [note, setNote] = useState(snippet.note);
  const [tags, setTags] = useState(snippet.tags.join(', '));
  const [kind, setKind] = useState<SnippetKind>(snippet.kind);
  const [busy, setBusy] = useState<null | 'save' | 'table' | 'latex'>(null);
  const [table, setTable] = useState<SnippetTable | null | 'none'>(null);
  const latex = useMemo(() => (snippet.latex ? renderLatex(snippet.latex) : null), [snippet.latex]);

  const startEdit = () => {
    setTitle(snippet.title);
    setNote(snippet.note);
    setTags(snippet.tags.join(', '));
    setKind(snippet.kind);
    setEditing(true);
  };

  const save = async () => {
    setBusy('save');
    try {
      const next = await researchApi.updateSnippet(snippet.id, { title: title.trim(), note: note.trim(), tags: parseTags(tags), kind });
      onChanged(next);
      setEditing(false);
    } catch (error) {
      notify.error('The snippet could not be saved', { description: toResearchError(error).message });
    } finally {
      setBusy(null);
    }
  };

  // Closed at once and deleted when the undo window ends; Undo shows it again.
  const remove = () => {
    const kept = snippet;
    removeWithUndo({
      message: 'Snippet deleted',
      description: snippetLabel(kept),
      hide: () => onChanged(null),
      restore: () => onChanged(kept),
      commit: () => researchApi.deleteSnippet(kept.id),
      onError: error => notify.error('The snippet could not be deleted', { description: toResearchError(error).message }),
    });
  };

  const extractTable = async () => {
    setBusy('table');
    try {
      const found = await researchApi.snippetTable(snippet.id);
      setTable(found ?? 'none');
    } catch (error) {
      notify.error('The table could not be read', { description: toResearchError(error).message });
    } finally {
      setBusy(null);
    }
  };

  const transcribe = async () => {
    if (!vision?.available) return;
    setBusy('latex');
    try {
      onChanged(await researchApi.transcribeSnippet(snippet.id));
    } catch (error) {
      notify.error('The equation could not be transcribed', { description: toResearchError(error).message });
    } finally {
      setBusy(null);
    }
  };

  const copyImage = async () => {
    if (image.status !== 'ready') return;
    try {
      await copyPngImage(image.png);
      notify.success('Image copied');
    } catch (error) {
      notify.error('The image could not be copied', { description: error instanceof Error ? error.message : String(error) });
    }
  };

  const copyRegionText = async () => {
    try {
      await copyText(snippet.latex?.trim() || snippet.text);
      notify.success(snippet.latex?.trim() ? 'LaTeX copied' : 'Text copied');
    } catch (error) {
      notify.error('The text could not be copied', { description: error instanceof Error ? error.message : String(error) });
    }
  };

  const visionDisabled = !vision?.available;
  const visionReason = vision === null ? 'Checking whether a vision-capable model is configured…' : vision.available ? null : vision.reason ?? 'No vision-capable model is configured.';
  const label = snippetLabel(snippet);

  return (
    <article className="w-full max-w-[860px] flex flex-col gap-4" aria-label={`Snippet: ${label}`}>
      <header className="flex flex-col gap-1">
        <p className="text-[11.5px] font-medium uppercase tracking-wider text-shodh-text-faint">
          {`${KIND_LABEL[snippet.kind]} · ${snippetSource(snippet)}`}
        </p>
        <h3 className="text-[17px] font-semibold text-shodh-text break-words">{snippet.title.trim() || label}</h3>
        {snippet.note.trim() && <p className="text-[13px] text-shodh-text-secondary whitespace-pre-wrap break-words">{snippet.note}</p>}
        {snippet.tags.length > 0 && (
          <ul className="flex flex-wrap gap-1.5" aria-label="Tags">
            {snippet.tags.map(tag => (
              <li key={tag} className="h-6 px-2 inline-flex items-center rounded-full bg-shodh-raised text-[11.5px] text-shodh-text-secondary">{tag}</li>
            ))}
          </ul>
        )}
      </header>

      <div className="rounded-xl border border-shodh-border bg-white overflow-hidden flex items-center justify-center min-h-[120px]">
        {image.status === 'ready' ? (
          <img
            src={pngDataUrl(image.png)}
            alt={`Region of ${snippetSource(snippet)}${snippet.text.trim() ? `: ${snippet.text.trim().slice(0, 160)}` : ''}`}
            className="max-w-full h-auto object-contain"
            draggable={false}
          />
        ) : image.status === 'loading' ? (
          <span role="status" className="flex items-center gap-2 py-10 text-[12.5px] text-zinc-600">
            <Loader2 className="w-4 h-4 animate-spin motion-reduce:animate-none" aria-hidden="true" />
            Drawing the region…
          </span>
        ) : (
          <span role="alert" className="flex items-start gap-2 p-4 text-[12.5px] text-zinc-700">
            <AlertTriangle className="w-4 h-4 mt-0.5 shrink-0 text-shodh-warning" aria-hidden="true" />
            {`The image could not be drawn: ${image.message}`}
          </span>
        )}
      </div>

      <div className="flex flex-wrap items-center gap-1.5" role="toolbar" aria-label="Snippet actions">
        <button type="button" className={BUTTON} onClick={() => void copyImage()} disabled={image.status !== 'ready'}>
          <ImageDown className="w-3.5 h-3.5" aria-hidden="true" />
          Copy image
        </button>
        <button type="button" className={BUTTON} onClick={() => void copyRegionText()} disabled={!snippet.text.trim() && !snippet.latex}>
          <ClipboardCopy className="w-3.5 h-3.5" aria-hidden="true" />
          {snippet.latex ? 'Copy LaTeX' : 'Copy text'}
        </button>
        <button type="button" className={BUTTON} onClick={() => insertIntoComposer(snippetChatContext(snippet))} title="Put this snippet in the Ask composer as context">
          <MessageSquarePlus className="w-3.5 h-3.5" aria-hidden="true" />
          Add to chat
        </button>
        {mode === 'dialog' && (
          <button
            type="button"
            className={BUTTON}
            onClick={() => showSourceBox({ filePath: snippet.filePath, fileName: snippet.fileName, page: snippet.page, rects: [{ page: snippet.page, rect: snippet.rect }], label })}
          >
            <FileSearch className="w-3.5 h-3.5" aria-hidden="true" />
            Show in paper
          </button>
        )}
        <button type="button" className={BUTTON} onClick={() => void extractTable()} disabled={busy === 'table'}>
          {busy === 'table' ? <Loader2 className="w-3.5 h-3.5 animate-spin motion-reduce:animate-none" aria-hidden="true" /> : <Table2 className="w-3.5 h-3.5" aria-hidden="true" />}
          Extract table
        </button>
        <button
          type="button"
          className={BUTTON}
          onClick={() => void transcribe()}
          aria-disabled={visionDisabled || busy === 'latex'}
          aria-describedby={visionReason ? visionReasonId : undefined}
          title={visionReason ?? `Transcribe with ${vision?.model ?? 'the configured model'}`}
        >
          {busy === 'latex' ? <Loader2 className="w-3.5 h-3.5 animate-spin motion-reduce:animate-none" aria-hidden="true" /> : <Sigma className="w-3.5 h-3.5" aria-hidden="true" />}
          Transcribe to LaTeX
        </button>
        <span className="flex-1" />
        <button type="button" className={BUTTON} onClick={editing ? () => setEditing(false) : startEdit} aria-expanded={editing}>
          <Pencil className="w-3.5 h-3.5" aria-hidden="true" />
          Edit
        </button>
        <button type="button" className={BUTTON} onClick={remove}>
          <Trash2 className="w-3.5 h-3.5" aria-hidden="true" />
          Delete
        </button>
      </div>
      {visionReason && (
        <p id={visionReasonId} className="-mt-2 text-[12px] text-shodh-text-muted">
          {`Transcribe to LaTeX is unavailable: ${visionReason}`}
        </p>
      )}

      {editing && (
        <form
          className="flex flex-col gap-3 rounded-xl border border-shodh-border bg-shodh-surface p-4"
          onSubmit={e => {
            e.preventDefault();
            void save();
          }}
          aria-label="Edit snippet"
        >
          <div className="grid gap-3 sm:grid-cols-[1fr_160px]">
            <div className="flex flex-col gap-1">
              <label htmlFor={titleId} className="text-[12px] font-medium text-shodh-text-secondary">Title</label>
              <input id={titleId} className={INPUT} value={title} maxLength={120} onChange={e => setTitle(e.target.value)} placeholder={label} />
            </div>
            <div className="flex flex-col gap-1">
              <label htmlFor={kindId} className="text-[12px] font-medium text-shodh-text-secondary">Kind</label>
              <select id={kindId} className={INPUT} value={kind} onChange={e => setKind(e.target.value as SnippetKind)}>
                {SNIPPET_KINDS.map(k => (
                  <option key={k} value={k}>{KIND_LABEL[k]}</option>
                ))}
              </select>
            </div>
          </div>
          <div className="flex flex-col gap-1">
            <label htmlFor={noteId} className="text-[12px] font-medium text-shodh-text-secondary">Note</label>
            <textarea id={noteId} className={cn(INPUT, 'h-auto min-h-[72px] py-2')} value={note} maxLength={2000} onChange={e => setNote(e.target.value)} />
          </div>
          <div className="flex flex-col gap-1">
            <label htmlFor={tagsId} className="text-[12px] font-medium text-shodh-text-secondary">Tags (comma separated)</label>
            <input id={tagsId} className={INPUT} value={tags} onChange={e => setTags(e.target.value)} />
          </div>
          <div className="flex items-center gap-2">
            <button type="submit" className={PRIMARY_BUTTON} disabled={busy === 'save'}>
              {busy === 'save' ? <Loader2 className="w-3.5 h-3.5 animate-spin motion-reduce:animate-none" aria-hidden="true" /> : <Check className="w-3.5 h-3.5" aria-hidden="true" />}
              Save
            </button>
            <button type="button" className={BUTTON} onClick={() => setEditing(false)}>
              <X className="w-3.5 h-3.5" aria-hidden="true" />
              Cancel
            </button>
          </div>
        </form>
      )}

      {snippet.latex && latex && (
        <section aria-labelledby={`${titleId}-latex`} className="flex flex-col gap-2">
          <h4 id={`${titleId}-latex`} className={SECTION_TITLE}>{`LaTeX · transcribed by ${snippet.latexModel ?? 'a vision model'}`}</h4>
          {'html' in latex ? (
            <div className="rounded-xl border border-shodh-border bg-shodh-surface px-4 py-3 overflow-x-auto text-shodh-text" dangerouslySetInnerHTML={{ __html: latex.html }} />
          ) : (
            <div role="alert" className="rounded-xl border border-shodh-border bg-shodh-surface px-4 py-3 flex flex-col gap-2">
              <p className="flex items-start gap-2 text-[12.5px] text-shodh-text-secondary">
                <AlertTriangle className="w-4 h-4 mt-0.5 shrink-0 text-shodh-warning" aria-hidden="true" />
                {`The transcription is not valid LaTeX (${latex.error}); shown as written.`}
              </p>
              <pre className="text-[12.5px] whitespace-pre-wrap break-words text-shodh-text font-mono">{snippet.latex}</pre>
            </div>
          )}
          <p className="text-[11.5px] text-shodh-text-faint">Machine transcription; check it against the image.</p>
        </section>
      )}

      {table !== null && (
        <section aria-label="Extracted table" className="flex flex-col gap-2">
          <div className="flex items-center gap-2">
            <h4 className={SECTION_TITLE}>Table from the parsed document</h4>
            {table !== 'none' && onOpenTable && (
              <button type="button" className={BUTTON} onClick={() => onOpenTable([table.header, ...table.rows].filter(r => r.length > 0))}>
                Open as table
              </button>
            )}
          </div>
          {table === 'none' ? (
            <p className="text-[12.5px] text-shodh-text-muted">
              The parsed document has no table overlapping this region. The parser may not have recognised it as a table; its text is below.
            </p>
          ) : (
            <TableBlock table={table} />
          )}
        </section>
      )}

      <section aria-label="Text of the region" className="flex flex-col gap-2">
        <h4 className={SECTION_TITLE}>Text in the region</h4>
        {snippet.text.trim() ? (
          <p className="text-[13.5px] leading-relaxed text-shodh-text-secondary whitespace-pre-wrap break-words">{snippet.text}</p>
        ) : (
          <p className="text-[12.5px] text-shodh-text-muted">This region has no text layer (a figure or a scanned page).</p>
        )}
      </section>
    </article>
  );
}
