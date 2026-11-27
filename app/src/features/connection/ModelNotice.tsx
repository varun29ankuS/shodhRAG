import { AlertTriangle, KeyRound, SlidersHorizontal } from 'lucide-react';
import { cn } from '../../lib/utils';
import { OPEN_MODEL_PICKER_EVENT } from '../modelPicker/modelApi';
import { requestFixKey } from '../modelPicker/navigation';
import { modelNotice } from './connectionStatus';
import { useConnectionSnapshot } from './useConnectionSnapshot';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1 focus-visible:ring-offset-shodh-surface';

const BUTTON = cn(
  'shrink-0 inline-flex items-center gap-1.5 h-7 px-2.5 rounded-md border border-shodh-border bg-shodh-surface text-caption font-medium text-shodh-text hover:bg-shodh-raised transition-colors duration-micro',
  FOCUS_RING,
);

/**
 * Above the message box: the model that would answer is known not to work now (its key was
 * rejected, it is out of quota, its server cannot be reached). Said before sending, in one
 * line, with the fix one click away (UI structure audit 2026-11-23, rule 6). Send stays
 * enabled: the user may know better, and a backup model may answer.
 */
export function ModelNotice({ className }: { className?: string }) {
  const { snapshot } = useConnectionSnapshot();
  const notice = modelNotice(snapshot);
  if (!notice) return null;
  return (
    <div
      role="status"
      data-testid="model-notice"
      data-tone={notice.tone}
      className={cn(
        'flex items-center gap-2 min-w-0 px-3 py-2 rounded-xl border text-small text-shodh-text',
        notice.tone === 'bad' ? 'border-shodh-error/40 bg-shodh-error/10' : 'border-shodh-warning/40 bg-shodh-warning-soft',
        className,
      )}
    >
      <AlertTriangle className={cn('w-4 h-4 shrink-0', notice.tone === 'bad' ? 'text-shodh-error' : 'text-shodh-warning')} aria-hidden="true" />
      <span className="flex-1 min-w-0 truncate" title={notice.text}>{notice.text}</span>
      <button type="button" className={BUTTON} onClick={() => requestFixKey(notice.provider)}>
        <KeyRound className="w-3.5 h-3.5" aria-hidden="true" />
        {notice.fix}
      </button>
      <button type="button" className={BUTTON} onClick={() => window.dispatchEvent(new CustomEvent(OPEN_MODEL_PICKER_EVENT))}>
        <SlidersHorizontal className="w-3.5 h-3.5" aria-hidden="true" />
        Change model
      </button>
    </div>
  );
}

export default ModelNotice;
