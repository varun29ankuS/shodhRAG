import React from 'react';
import AuditView from '../audit/AuditView';
import { PageFrame } from '../../components/shell/PageFrame';

/**
 * Activity: usage and the tamper-evident audit log — what was asked, which
 * tools ran, what was retrieved and what changed on this computer. The log's
 * exports are the page's primary action, in the frame's slot.
 */
export default function ActivityView() {
  return (
    <AuditView
      frame={(action, body) => (
        <PageFrame title="Activity" lede="A tamper-evident log, kept on this computer" action={action}>
          {body}
        </PageFrame>
      )}
    />
  );
}
