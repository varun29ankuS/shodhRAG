/**
 * How the model picker presents models: prices, context length, privacy,
 * and the grouped, searchable list (Favourites, Recent, Free, Paid, Local).
 *
 * Pure module (no React), unit-tested with Node (`app/tests/modelPicker.test.ts`).
 */
import type { ActiveModel, CatalogModel, ModelRef, ModelPrefs, PickId, PickerView, PrivacyNote, ProviderId } from './modelTypes.ts';
import { PROVIDER_LABELS, sameModel } from './modelTypes.ts';

/** USD per million tokens as shown: "$0.18", "$3", "$15", "<$0.01"; null is unknown. */
export function formatUsd(perMillion: number | null): string | null {
  if (perMillion === null || !Number.isFinite(perMillion) || perMillion < 0) return null;
  if (perMillion === 0) return '$0';
  if (perMillion < 0.01) return '<$0.01';
  if (perMillion >= 100) return `$${Math.round(perMillion)}`;
  const fixed = perMillion.toFixed(2);
  return `$${fixed.endsWith('.00') ? fixed.slice(0, -3) : fixed}`;
}

/** The price line of a model: "Free", "On this device", "Included in your plan", "$1 in · $5 out per 1M tokens" or "Price unknown". */
export function formatPrice(model: Pick<CatalogModel, 'tier' | 'promptPerMillion' | 'completionPerMillion'>): string {
  if (model.tier === 'local') return 'On this device';
  if (model.tier === 'subscription') return 'Included in your plan';
  if (model.tier === 'free') return 'Free';
  const input = formatUsd(model.promptPerMillion);
  const output = formatUsd(model.completionPerMillion);
  if (input === null || output === null) return 'Price unknown';
  return `${input} in · ${output} out per 1M tokens`;
}

/** Context window as "128K", "1M", "8,192"; null when unknown. */
export function formatContext(tokens: number | null): string | null {
  if (tokens === null || !Number.isFinite(tokens) || tokens <= 0) return null;
  if (tokens >= 1_000_000) {
    const m = tokens / 1_000_000;
    return `${Number.isInteger(m) ? m : m.toFixed(1)}M`;
  }
  if (tokens >= 10_000) return `${Math.round(tokens / 1_000)}K`;
  return tokens.toLocaleString('en-US');
}

/** One sentence on what happens to prompts sent to the model. */
export function privacyText(privacy: PrivacyNote, stealth: boolean): string {
  if (stealth) return 'Stealth model: its provider may log prompts and use them for training.';
  switch (privacy) {
    case 'may_log_prompts':
      return 'Free endpoint: the provider may log prompts.';
    case 'provider_terms':
      return "Sent to the provider under its API terms.";
    case 'on_device':
      return 'Stays on this computer.';
  }
}

/** Where the active model was set, for the chip and Settings → Model. */
export function sourceText(active: ActiveModel | null): string | null {
  if (!active) return null;
  switch (active.source) {
    case 'environment':
      return 'Set by environment';
    case 'session':
      return 'This session only';
    case 'settings':
      return null;
  }
}

/** The catalog entry of a model reference, if listed. */
export function findModel(models: readonly CatalogModel[], ref: ModelRef | null | undefined): CatalogModel | null {
  if (!ref) return null;
  return models.find(m => m.provider === ref.provider && m.id === ref.model) ?? null;
}

/** A short display name: the catalog's name, else the id after the vendor prefix. */
export function modelName(models: readonly CatalogModel[], ref: ModelRef | null | undefined): string {
  if (!ref) return 'No model';
  const listed = findModel(models, ref);
  if (listed) return listed.name;
  return ref.model;
}

export function modelKey(ref: ModelRef): string {
  return `${ref.provider}:${ref.model}`;
}

export type SectionId = 'favourites' | 'recent' | 'subscription' | 'free' | 'paid' | 'local';

export interface PickerSection {
  id: SectionId;
  title: string;
  models: CatalogModel[];
}

const SECTION_TITLES: Record<SectionId, string> = {
  favourites: 'Favourites',
  recent: 'Recent',
  subscription: 'Your plans',
  free: 'Free',
  paid: 'Paid',
  local: 'Local',
};

/** Whether `model` matches every word of `query` (name, id or provider). */
export function matchesQuery(model: CatalogModel, query: string): boolean {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  if (words.length === 0) return true;
  const haystack = `${model.name} ${model.id} ${PROVIDER_LABELS[model.provider]}`.toLowerCase();
  return words.every(w => haystack.includes(w));
}

function byName(a: CatalogModel, b: CatalogModel): number {
  return a.name.localeCompare(b.name, 'en', { sensitivity: 'base' }) || a.id.localeCompare(b.id);
}

/**
 * The picker's sections. Favourites and Recent come first (each model listed
 * once: a favourite is not repeated under Recent); the rest are grouped by
 * tier. While searching, Favourites and Recent are left out so every match
 * appears once, in its tier.
 */
export function pickerSections(models: readonly CatalogModel[], prefs: ModelPrefs, query: string): PickerSection[] {
  const matching = models.filter(m => matchesQuery(m, query));
  const sections: PickerSection[] = [];
  const searching = query.trim().length > 0;
  if (!searching) {
    const favourites = prefs.favourites.map(ref => findModel(matching, ref)).filter((m): m is CatalogModel => m !== null);
    const recent = prefs.recent
      .filter(ref => !prefs.favourites.some(f => sameModel(f, ref)))
      .map(ref => findModel(matching, ref))
      .filter((m): m is CatalogModel => m !== null);
    if (favourites.length > 0) sections.push({ id: 'favourites', title: SECTION_TITLES.favourites, models: favourites });
    if (recent.length > 0) sections.push({ id: 'recent', title: SECTION_TITLES.recent, models: recent });
  }
  for (const tier of ['subscription', 'free', 'paid', 'local'] as const) {
    const inTier = matching.filter(m => m.tier === tier).sort(byName);
    if (inTier.length > 0) sections.push({ id: tier, title: SECTION_TITLES[tier], models: inTier });
  }
  return sections;
}

/** Why the model list may be incomplete or stale, as one line (null when it is current). */
export function catalogNotice(view: Pick<PickerView, 'catalogStatus' | 'catalogFetchedAtMs' | 'keyed' | 'ollamaRunning'>, nowMs: number): string | null {
  switch (view.catalogStatus) {
    case 'fresh':
      return null;
    case 'local_only':
      return 'Local-only mode is on: only models on this computer are listed.';
    case 'unavailable':
      return view.keyed.includes('openrouter')
        ? 'The OpenRouter model list could not be fetched (offline?). Prices are unknown until it is.'
        : null;
    case 'cached': {
      if (view.catalogFetchedAtMs === null) return 'Showing a saved model list.';
      const hours = Math.max(1, Math.round((nowMs - view.catalogFetchedAtMs) / 3_600_000));
      return `Offline: prices from a list saved ${hours === 1 ? 'an hour' : `${hours} hours`} ago.`;
    }
  }
}

/** One of the four picks, as described under its name. */
export const PICK_HINTS: Record<PickId, string> = {
  best: 'The strongest model you have connected.',
  fast: 'Quick answers that cost less.',
  free: 'No cost. The provider may log prompts.',
  private: 'Runs on this computer. Nothing leaves it.',
};

/** Why a pick has no model, for its disabled row. */
export function pickUnavailableText(pick: PickId, view: Pick<PickerView, 'localOnly' | 'lmstudio'>): string {
  switch (pick) {
    case 'free':
      return view.localOnly ? 'Off in Local-only mode.' : 'Connect OpenRouter for free models.';
    case 'private':
      return view.lmstudio.running ? 'Load a model in LM Studio.' : 'Start LM Studio to run a model here.';
    default:
      return view.localOnly ? 'Off in Local-only mode.' : 'Connect a provider above.';
  }
}

/** A row of the composer's model menu. */
export interface ChipEntry {
  kind: 'pick' | 'recent';
  /** For picks. */
  pick: PickId | null;
  label: string;
  model: ModelRef;
  name: string;
}

/**
 * The composer chip's menu: the picks that resolved, then recently used
 * models of connected providers that are not already a pick (at most
 * `maxRecent`).
 */
export function chipEntries(
  view: Pick<PickerView, 'picks' | 'prefs' | 'models' | 'connected'>,
  maxRecent = 4,
): ChipEntry[] {
  const entries: ChipEntry[] = [];
  for (const option of view.picks) {
    if (!option.model) continue;
    entries.push({ kind: 'pick', pick: option.pick, label: option.label, model: option.model, name: option.name ?? option.model.model });
  }
  let recent = 0;
  for (const ref of view.prefs.recent) {
    if (recent >= maxRecent) break;
    if (!(view.connected as readonly ProviderId[]).includes(ref.provider)) continue;
    if (entries.some(e => sameModel(e.model, ref))) continue;
    entries.push({ kind: 'recent', pick: null, label: 'Recent', model: ref, name: modelName(view.models, ref) });
    recent += 1;
  }
  return entries;
}
