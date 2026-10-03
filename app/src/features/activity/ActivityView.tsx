import React from 'react';
import AuditView from '../audit/AuditView';

/**
 * Activity: usage and the tamper-evident audit log — what was asked, which
 * tools ran, what was retrieved and what changed on this computer.
 */
export default function ActivityView() {
  return (
    <div className="h-full overflow-y-auto bg-shodh-ground text-shodh-text">
      <div className="max-w-[1080px] mx-auto px-8 py-7 flex flex-col gap-6">
        <header className="flex flex-col gap-1.5">
          <h1 className="m-0 text-2xl font-bold">Activity</h1>
          <p className="text-sm text-shodh-text-muted">
            What was asked, which tools ran, what was retrieved and what changed, in a tamper-evident log on this
            computer.
          </p>
        </header>
        <AuditView />
      </div>
    </div>
  );
}
