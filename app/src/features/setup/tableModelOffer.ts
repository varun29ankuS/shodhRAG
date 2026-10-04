/** Emitted by the backend (`table_model_commands.rs`) at most once per run. */
export const TABLE_MODEL_SUGGESTED_EVENT = 'table-model-suggested';
/** Set once the suggestion was shown, so it is not repeated in later runs. */
export const PROMPT_SHOWN_KEY = 'shodh.tableModelPromptShown';

/**
 * Whether the table model offer should be shown now, recording that it was:
 * true the first time only. Storage that cannot be read or written never blocks
 * the offer (it may then show again in a later run).
 */
export function claimTableModelOffer(storage: Pick<Storage, 'getItem' | 'setItem'> | null): boolean {
  let shown = false;
  try {
    shown = storage?.getItem(PROMPT_SHOWN_KEY) === '1';
  } catch {
    shown = false;
  }
  if (shown) return false;
  try {
    storage?.setItem(PROMPT_SHOWN_KEY, '1');
  } catch {
    // Without storage the offer may repeat in a later run; nothing breaks.
  }
  return true;
}
