import { useMemo } from 'react';
import { useModelPicker } from './modelApi';
import { modelName } from './modelFormat';
import type { PickerView } from './modelTypes';

/** The model control's text: the active model's name, or what to do when there is none. */
export function modelLabelOf(view: PickerView | null): { label: string; ready: boolean } {
  const active = view?.active ?? null;
  if (active) return { label: modelName(view?.models ?? [], active.model), ready: true };
  return { label: view ? 'Choose a model' : 'Model', ready: false };
}

/**
 * The model the next answer uses, for a composer that cannot open the model menu itself
 * (the floating chat over the preview). Read while `active`, and again on every model
 * change anywhere in the app.
 */
export function useModelLabel(active: boolean): { label: string; ready: boolean } {
  const { view } = useModelPicker(active);
  const { label, ready } = modelLabelOf(view);
  return useMemo(() => ({ label, ready }), [label, ready]);
}
