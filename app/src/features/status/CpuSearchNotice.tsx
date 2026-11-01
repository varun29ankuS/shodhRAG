/**
 * The notice where indexing happens (the Library) while search runs on the
 * CPU and is not fast: the measured speed, the reason, and what helps. Not
 * shown on a graphics card, before the first measurement, or on a CPU
 * measured fast.
 */
import { Cpu } from 'lucide-react';
import { cn } from '../../lib/utils';
import { useSystemStatusContext } from './SystemStatusContext';
import { cpuNotice } from './systemStatus';

export function CpuSearchNotice() {
  const system = useSystemStatusContext();
  const status = system?.status ?? null;
  const notice = cpuNotice(status);
  if (!notice || status?.search?.speed === 'fast') return null;
  return (
    <div
      role="status"
      className={cn(
        'rounded-xl border px-4 py-3 flex items-start gap-3',
        notice.tone === 'bad' ? 'border-shodh-error/40 bg-shodh-error/10' : 'border-shodh-warning/40 bg-shodh-warning-soft',
      )}
    >
      <Cpu className="w-4 h-4 mt-0.5 shrink-0 text-shodh-text-muted" aria-hidden="true" />
      <div className="flex flex-col gap-0.5 min-w-0">
        <p className="m-0 text-small font-semibold text-shodh-text">{notice.title}</p>
        <p className="m-0 text-small text-shodh-text-secondary">{notice.reason}</p>
        <p className="m-0 text-small text-shodh-text-secondary">{notice.advice}</p>
      </div>
    </div>
  );
}

export default CpuSearchNotice;
