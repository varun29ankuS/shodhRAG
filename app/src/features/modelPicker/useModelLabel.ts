import { useCallback, useMemo, useRef } from 'react';
import { useModelPicker } from './modelApi';
import { modelDot } from '../connection/connectionStatus.ts';
import { modelLabelOf } from './modelFormat';
import { useConnectionSnapshot } from '../connection/useConnectionSnapshot';
import type { DockModelLabel } from '../../lib/dockProtocol';
import { applyDockChoice, dockModelChoices } from './dockChoices';
import type { DockChoiceResult } from './dockChoices';

/**
 * The model control of a chat that shows main's state rather than reading the models itself
 * (the floating chat, in the page or over the preview): the model's name, the colour of its
 * state, and the rows of its menu, with `choose` to apply one of them. Read while `active`,
 * and again on every model change anywhere in the app.
 */
export function useDockModel(active: boolean): { model: DockModelLabel; choose: (id: string) => Promise<DockChoiceResult> } {
  const picker = useModelPicker(active);
  const { snapshot } = useConnectionSnapshot();
  const { view } = picker;
  const { label, ready } = modelLabelOf(view);
  const dot = modelDot(snapshot);
  const choices = useMemo(() => dockModelChoices(view), [view]);
  const model = useMemo(() => ({ label, ready, dot, choices }), [label, ready, dot, choices]);
  const pickerRef = useRef(picker);
  pickerRef.current = picker;
  const choose = useCallback((id: string) => applyDockChoice(id, pickerRef.current), []);
  return { model, choose };
}
