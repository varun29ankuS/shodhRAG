/**
 * What waits for the computer: each queued question, file or edit with
 * where it is (waiting, sending, sent, answered, or why it was not sent),
 * and Remove for one not yet sent or one that failed.
 */
import { CircleCheck, Clock, LoaderCircle, Send, Trash2, TriangleAlert } from 'lucide-react';
import { itemWords, type QueuedItem } from '../lib/outbox';
import { useQueue } from '../lib/queue';

function StateIcon({ item }: { item: QueuedItem }) {
  switch (item.state) {
    case 'waiting':
      return <Clock aria-hidden="true" className="icon-small" />;
    case 'sending':
      return <LoaderCircle aria-hidden="true" className="icon-small spin" />;
    case 'sent':
      return <Send aria-hidden="true" className="icon-small" />;
    case 'answered':
      return <CircleCheck aria-hidden="true" className="icon-small" />;
    case 'failed':
      return <TriangleAlert aria-hidden="true" className="icon-small" />;
  }
}

export function QueuedRows({ items }: { items: QueuedItem[] }) {
  const { remove } = useQueue();
  if (items.length === 0) return null;
  return (
    <div className="rows" aria-label="Waiting for your computer">
      {items.map(item => {
        const words = itemWords(item);
        const removable = item.state === 'waiting' || item.state === 'failed';
        return (
          <div key={item.id} className="row queued" data-state={item.state}>
            <span className="row-icon">
              <StateIcon item={item} />
            </span>
            <span className="row-text">
              <span className="row-title">{words.title}</span>
              <span className="row-sub" role={item.state === 'sending' ? 'status' : undefined}>
                {words.state}
              </span>
            </span>
            {removable && (
              <button type="button" className="icon-button" aria-label={`Remove “${words.title}”`} onClick={() => remove(item.id).catch(() => undefined)}>
                <Trash2 aria-hidden="true" className="icon-small" />
              </button>
            )}
          </div>
        );
      })}
    </div>
  );
}
