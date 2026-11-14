/**
 * "Send to Shodh": files shared from another app, added in the Library or
 * photographed, and where they go: "From my phone" (the default) or one of
 * the user's workspaces. They are kept on the phone and sent in order; the
 * computer applies its upload rules and says why it skipped any.
 */
import { useState } from 'react';
import { FileUp, FolderInput, Send, X } from 'lucide-react';
import { useLink, useNote } from '../lib/context';
import type { Picked } from '../lib/device';
import { workspacesList } from '../lib/desktop';
import { useOffers } from '../lib/incoming';
import { errorText } from '../lib/link';
import { useDevice, useQueue } from '../lib/queue';
import { useRemote } from '../lib/remote';
import { Sheet } from './Sheet';

/** "From my phone": the inbox, the default. */
export const INBOX = 'From my phone';

export function SendSheet({ files }: { files: Picked[] }) {
  const { connected } = useLink();
  const { closeSend } = useOffers();
  const device = useDevice();
  const queue = useQueue();
  const { show } = useNote();
  const workspaces = useRemote('workspaces', workspacesList);
  const [where, setWhere] = useState<string | null>(null);
  const [sending, setSending] = useState(false);

  const cancel = () => {
    for (const file of files) device.files.discard(file.path).catch(() => undefined);
    closeSend();
  };

  const send = async () => {
    setSending(true);
    try {
      for (const file of files) {
        const details = await device.files.details(file, where);
        await queue.add(details);
      }
      show(connected ? 'Sending to your computer.' : 'Saved on your phone. Sent when your computer wakes.');
      closeSend();
    } catch (e) {
      show(errorText(e), 'problem');
      setSending(false);
    }
  };

  const choices: Array<{ id: string | null; name: string; note: string }> = [
    { id: null, name: INBOX, note: 'A folder of its own in your Library' },
    ...(workspaces.data ?? []).map(w => ({ id: w.id, name: w.name, note: 'Searched in this workspace\u2019s chats' })),
  ];
  const label = files.length === 1 ? files[0].name : `${files.length} files`;

  return (
    <Sheet label="Send to Shodh" onClose={cancel}>
      <span className="approval-context">Send to Shodh on your computer</span>
      <div className="sheet-card">
        <span className="sheet-card-title">
          <FileUp aria-hidden="true" className="icon-small" /> {label}
        </span>
        {files.length > 1 && <span className="sheet-card-text">{files.map(f => f.name).join(', ')}</span>}
      </div>
      <div className="label">Where it goes</div>
      <div className="choices" role="radiogroup" aria-label="Where it goes">
        {choices.map(choice => (
          <button
            key={choice.id ?? 'inbox'}
            type="button"
            role="radio"
            className="choice"
            aria-checked={where === choice.id}
            onClick={() => setWhere(choice.id)}
          >
            <FolderInput aria-hidden="true" className="icon-small" />
            <span className="row-text">
              <span className="row-title">{choice.name}</span>
              <span className="row-sub">{choice.note}</span>
            </span>
          </button>
        ))}
      </div>
      <p className="panel-text">Shodh on your computer adds it as it adds any file: files that may hold secrets, and files too large to read, are kept back.</p>
      <div className="sheet-actions">
        <button type="button" className="button" onClick={cancel} disabled={sending}>
          <X aria-hidden="true" className="icon-small" />
          Cancel
        </button>
        <button type="button" className="button button-primary" onClick={() => void send()} disabled={sending}>
          <Send aria-hidden="true" className="icon-small" />
          {connected ? 'Send' : 'Send when it wakes'}
        </button>
      </div>
    </Sheet>
  );
}
