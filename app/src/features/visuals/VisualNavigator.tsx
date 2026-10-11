import { useCallback } from 'react';
import { notify } from '../../lib/notify';
import { useNavigationTarget } from '../agent/useNavigationTarget';
import type { VisualTarget } from '../agent/events';
import { useFocus } from '../focus/FocusContext';
import { toVisualError, visualsApi } from './api';
import { recordTarget, visualRef } from './model';

/**
 * Opens the visuals the agent shows (`open_visual`) in the focus pop-out.
 * Rendered once inside the focus provider.
 */
export function VisualNavigator() {
  const focus = useFocus();
  const open = useCallback((target: VisualTarget) => {
    if (!focus) return;
    const trigger = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    visualsApi.get(target.visualId, target.version ? { version: target.version } : { latest: true })
      .then(detail => {
        const record = detail.visual;
        const focusTarget = recordTarget(record);
        if (!focusTarget) {
          notify.error('This visual cannot be drawn');
          return;
        }
        focus.openFocus({
          target: focusTarget,
          conversationId: record.conversationId,
          parentMessageId: record.messageId,
          trigger,
          visual: visualRef(record),
        });
      })
      .catch(error => notify.error('The visual could not be opened', { description: toVisualError(error).message }));
  }, [focus]);
  useNavigationTarget('visual', open);
  return null;
}
