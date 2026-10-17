/**
 * Opening the model UI from elsewhere (the sidebar's model status, an
 * answer's error card): the connect UI of one provider ("Fix key"), or the
 * Ask composer's model picker ("Switch model"). Each request is kept until
 * the UI takes it, so it works whether that UI is mounted already (event)
 * or mounts after the navigation (initial state).
 */
import type { ProviderId } from './modelTypes';
import { requestSettingsSection } from '../memory/navigation';

/** Window event: a provider's connect UI was requested (see takeFixKeyRequest). */
export const FIX_KEY_EVENT = 'shodh:fix-provider-key';
/** Window event: the Ask composer's model picker was requested. */
export const OPEN_PICKER_REQUEST_EVENT = 'shodh:open-model-picker-request';

let fixKeyFor: ProviderId | null = null;
let pickerRequested = false;

function showView(view: 'settings' | 'ask'): void {
  window.dispatchEvent(new CustomEvent('switchTab', { detail: view }));
}

/** Show Settings → Model with `provider`'s connect UI (its key, sign-in or local server). */
export function requestFixKey(provider: ProviderId): void {
  fixKeyFor = provider;
  requestSettingsSection('models');
  showView('settings');
  window.dispatchEvent(new CustomEvent(FIX_KEY_EVENT));
}

/** The provider whose connect UI was requested, once. */
export function takeFixKeyRequest(): ProviderId | null {
  const provider = fixKeyFor;
  fixKeyFor = null;
  return provider;
}

/** Show Ask with its model picker open. */
export function requestModelPicker(): void {
  pickerRequested = true;
  showView('ask');
  window.dispatchEvent(new CustomEvent(OPEN_PICKER_REQUEST_EVENT));
}

/** Whether the model picker was requested, once. */
export function takeModelPickerRequest(): boolean {
  const requested = pickerRequested;
  pickerRequested = false;
  return requested;
}
