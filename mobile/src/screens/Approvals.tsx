/**
 * Approvals: the steps the assistant is waiting on, from the computer,
 * including ones asked before this phone connected. A step the phone may
 * allow (Ask and Creator steps) opens a sheet to allow or decline it; any
 * other step is approved on the computer, and says so.
 */
import { useState } from 'react';
import { motion } from 'framer-motion';
import { Monitor } from 'lucide-react';
import { TopBar } from '../components/Bars';
import { NotConnected } from '../components/Connection';
import { Sheet } from '../components/Sheet';
import { useLink, useNav, useNote } from '../lib/context';
import { useData } from '../lib/data';
import { approve, type ConversationSummary, type PendingApproval } from '../lib/desktop';
import { errorText } from '../lib/link';
import { viewIn } from '../lib/motion';
import type { SheetTarget } from '../lib/nav';
import { ago } from '../lib/time';

function context(approval: PendingApproval, conversations: ConversationSummary[] | undefined, now: Date): string {
  const conversation = conversations?.find(c => c.id === approval.conversationId);
  const when = ago(approval.requestedAt, now);
  return conversation ? `${conversation.title}, ${when.toLowerCase()}` : when;
}

export function Approvals({ now = () => new Date() }: { now?: () => Date }) {
  const { connected } = useLink();
  const { dispatch } = useNav();
  const { approvals, conversations } = useData();
  const today = now();
  const list = approvals.data;

  return (
    <motion.div className="view" variants={viewIn} initial="hidden" animate="shown" exit="gone">
      <div className="scroll">
        <TopBar brand={false} />
        <h1 className="title serif pad">Approvals</h1>
        {list === undefined && !connected && <NotConnected what="what the assistant is waiting on" />}
        {list === undefined && connected && (
          <div className="skeleton-lines" aria-busy="true" aria-label="Loading approvals">
            <div className="skeleton skeleton-line" style={{ width: '50%' }} />
            <div className="skeleton skeleton-line" style={{ width: '88%', height: 24 }} />
            <div className="skeleton" style={{ width: 120, height: 44, borderRadius: 22 }} />
          </div>
        )}
        {list !== undefined && list.length === 0 && (
          <p className="panel-text serif pad" style={{ marginTop: 20, fontSize: 19 }}>
            Nothing needs you right now. When the assistant wants a yes, it asks here.
          </p>
        )}
        {list !== undefined && list.length > 0 && (
          <div className="pad">
            {list.map(approval => (
              <article key={`${approval.sessionId}:${approval.stepId}`} className="approval">
                <span className="approval-context">{context(approval, conversations.data, today)}</span>
                <span className="approval-title serif">{approval.title}</span>
                {approval.phoneMay && approval.sessionId && approval.stepId ? (
                  <button
                    type="button"
                    className="button button-primary"
                    onClick={() =>
                      approval.sessionId &&
                      approval.stepId &&
                      dispatch({ type: 'sheet', target: { sessionId: approval.sessionId, stepId: approval.stepId } })
                    }
                  >
                    Review
                  </button>
                ) : (
                  <span className="on-computer">
                    <Monitor aria-hidden="true" className="icon" />
                    <span>Approve this on your computer</span>
                  </span>
                )}
              </article>
            ))}
          </div>
        )}
      </div>
    </motion.div>
  );
}

/** The sheet to allow or decline one step. */
export function ApprovalSheet({ target }: { target: SheetTarget }) {
  const { desktop } = useLink();
  const { back } = useNav();
  const { show } = useNote();
  const { approvals, conversations } = useData();
  const [sending, setSending] = useState(false);
  const approval = approvals.data?.find(a => a.sessionId === target.sessionId && a.stepId === target.stepId) ?? null;

  const answer = (approved: boolean) => {
    setSending(true);
    approve(desktop, target.sessionId, target.stepId, approved)
      .then(() => {
        show(approved ? 'Allowed. Your computer goes on.' : 'Declined. The assistant was told.');
        back();
        approvals.reload();
      })
      .catch(error => {
        show(errorText(error), 'problem');
        setSending(false);
      });
  };

  return (
    <Sheet label="Approve a step" onClose={back}>
      <span className="approval-context">
        {approval ? context(approval, conversations.data, new Date()) : 'The assistant is waiting for your answer'}
      </span>
      <div className="sheet-card">
        <span className="sheet-card-title serif">{approval?.title ?? 'Approve a step'}</span>
        <span className="sheet-card-text serif">
          Allow it and the assistant goes on with this step on your computer. Decline it and the assistant is told, and asks how to continue.
        </span>
      </div>
      {approval && !approval.phoneMay ? (
        <span className="on-computer">
          <Monitor aria-hidden="true" className="icon" />
          <span>Approve this on your computer</span>
        </span>
      ) : (
        <div className="sheet-actions">
          <button type="button" className="button" disabled={sending} onClick={() => answer(false)}>
            Decline
          </button>
          <button type="button" className="button button-primary" disabled={sending} onClick={() => answer(true)}>
            Allow
          </button>
        </div>
      )}
    </Sheet>
  );
}
