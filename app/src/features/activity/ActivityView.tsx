import React from 'react';
import AuditView from '../audit/AuditView';
import { PageFrame } from '../../components/shell/PageFrame';

/**
 * Activity: usage and the tamper-evident audit log — what was asked, which
 * tools ran, what was retrieved and what changed on this computer.
 */
export default function ActivityView() {
  return (
    <PageFrame title="Activity" lede="A tamper-evident log, kept on this computer">
      <AuditView />
    </PageFrame>
  );
}
