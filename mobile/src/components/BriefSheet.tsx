/** The brief of one event: what was discussed last time, and what the library has on it, cited. */
import { FileText, MessageSquareText, NotebookText } from 'lucide-react';
import { loadBrief, type BriefSource } from '../lib/brief';
import { useNav } from '../lib/context';
import { firstPage } from '../lib/answer';
import { useRemote } from '../lib/remote';
import { ago } from '../lib/time';
import { Sheet } from './Sheet';

export function BriefSheet({ eventId, title, onClose }: { eventId: string; title: string; onClose: () => void }) {
  const { dispatch } = useNav();
  const brief = useRemote(`brief:${eventId}`, desktop => loadBrief(desktop, eventId));
  const open = (source: BriefSource) => {
    if (source.kind === 'note') return;
    onClose();
    dispatch({ type: 'read', target: { path: source.path, name: source.file, page: firstPage(source.page), passage: source.text, back: 'Tasks', origin: null } });
  };
  return (
    <Sheet label={`Brief: ${title}`} onClose={onClose}>
      <span className="approval-context">
        <NotebookText aria-hidden="true" className="icon-small" /> {`Before ${title}`}
      </span>
      {brief.data === undefined && brief.error === null && (
        <div className="skeleton-lines" aria-busy="true" aria-label="Loading the brief">
          <div className="skeleton skeleton-line" style={{ width: '86%' }} />
          <div className="skeleton skeleton-line" style={{ width: '60%' }} />
        </div>
      )}
      {brief.error !== null && <p className="panel-text">{brief.error}</p>}
      {brief.data === null && <p className="panel-text">Nothing in your library or your conversations is about this event.</p>}
      {brief.data && (
        <>
          {brief.data.conversations.length > 0 && (
            <>
              <div className="label">Last time</div>
              <div className="rows rows-icons">
                {brief.data.conversations.map(c => (
                  <button
                    key={c.id}
                    type="button"
                    className="row"
                    onClick={() => {
                      onClose();
                      dispatch({ type: 'push', screen: { kind: 'answer', conversationId: c.id, question: null, workspaceId: null } });
                    }}
                  >
                    <span className="row-icon">
                      <MessageSquareText aria-hidden="true" className="icon-small" />
                    </span>
                    <span className="row-text">
                      <span className="row-title">{c.title}</span>
                      <span className="row-sub">{ago(c.updatedAt, new Date())}</span>
                    </span>
                  </button>
                ))}
              </div>
            </>
          )}
          {brief.data.sources.length > 0 && (
            <>
              <div className="label">From your library</div>
              <div className="rows">
                {brief.data.sources.map(source => (
                  <button key={source.n} type="button" className="row" onClick={() => open(source)}>
                    <span className="chip" aria-hidden="true">
                      {source.n}
                    </span>
                    <span className="row-text">
                      <span className="row-title">
                        {source.kind === 'note' ? <NotebookText aria-hidden="true" className="icon-small" /> : <FileText aria-hidden="true" className="icon-small" />} {source.file}
                        {source.page ? `, page ${source.page}` : ''}
                      </span>
                      <span className="row-sub">{source.text}</span>
                    </span>
                  </button>
                ))}
              </div>
            </>
          )}
        </>
      )}
    </Sheet>
  );
}
