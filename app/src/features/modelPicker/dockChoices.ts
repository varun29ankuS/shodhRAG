/**
 * The floating chat's model menu. The chat over the preview runs in a webview of its own
 * with no model list, so main offers it the rows of the composer chip's menu
 * (`chipEntries`: the picks that resolve and the recent models) by id, and applies a pick
 * only when the id is one of the rows it offers now. Pure, so the Node tests load it.
 */
import type { DockModelChoice } from '../../lib/dockProtocol.ts';
import { MAX_CHOICE_TEXT, MAX_MODEL_CHOICES, MODEL_CHOICE_ID } from '../../lib/dockProtocol.ts';
import type { ChipEntry } from './modelFormat.ts';
import { chipEntries, chipEntryActive, chipEntryDetail, chipEntryTitle } from './modelFormat.ts';
import type { ModelRef, PickId, PickerError, PickerView } from './modelTypes.ts';

/** A row's id: its pick, or its model. Null when it cannot be sent as one (it stays in Ask's menu). */
export function choiceId(entry: ChipEntry): string | null {
  const id = entry.kind === 'pick' && entry.pick ? `pick:${entry.pick}` : `model:${entry.model.provider}:${entry.model.model}`;
  return MODEL_CHOICE_ID.test(id) ? id : null;
}

function short(text: string): string {
  return text.length > MAX_CHOICE_TEXT ? `${text.slice(0, MAX_CHOICE_TEXT - 1)}…` : text;
}

type ChoiceView = Pick<PickerView, 'picks' | 'prefs' | 'models' | 'connected' | 'active'>;

/** The rows offered now, with their ids. */
function offered(view: ChoiceView): Array<{ id: string; entry: ChipEntry }> {
  const rows: Array<{ id: string; entry: ChipEntry }> = [];
  for (const entry of chipEntries(view)) {
    const id = choiceId(entry);
    if (id === null || rows.some(r => r.id === id)) continue;
    rows.push({ id, entry });
    if (rows.length === MAX_MODEL_CHOICES) break;
  }
  return rows;
}

/** The menu's rows as the floating chat shows them; none until the models are read. */
export function dockModelChoices(view: ChoiceView | null): DockModelChoice[] {
  if (!view) return [];
  return offered(view).map(({ id, entry }) => ({
    id,
    title: short(chipEntryTitle(entry)),
    detail: short(chipEntryDetail(entry)),
    selected: chipEntryActive(view, entry),
  }));
}

/** What choosing a row did. */
export type DockChoiceResult =
  /** The id is not one of the rows offered now (the list changed, or it was never one). */
  | { kind: 'unknown' }
  /** It already answers. */
  | { kind: 'same'; name: string }
  | { kind: 'changed'; name: string }
  | { kind: 'refused'; name: string; error: PickerError };

/** What applying a choice needs of the model picker (`useModelPicker`). */
export interface ChoicePicker {
  view: ChoiceView | null;
  select: (model: ModelRef) => Promise<PickerError | null>;
  choosePick: (pick: PickId) => Promise<PickerError | null>;
}

/**
 * Choose the row `id`, as the same click in Ask's menu does. Only a row offered now is
 * chosen, and nothing a row in Ask would ask to confirm first is confirmed here: a refusal
 * (the environment sets the model, a key is missing) is returned for main to show.
 */
export async function applyDockChoice(id: string, picker: ChoicePicker): Promise<DockChoiceResult> {
  const view = picker.view;
  const row = view ? offered(view).find(r => r.id === id) : undefined;
  if (!view || !row) return { kind: 'unknown' };
  const { entry } = row;
  if (chipEntryActive(view, entry)) return { kind: 'same', name: entry.name };
  const error = entry.kind === 'pick' && entry.pick ? await picker.choosePick(entry.pick) : await picker.select(entry.model);
  return error ? { kind: 'refused', name: entry.name, error } : { kind: 'changed', name: entry.name };
}
