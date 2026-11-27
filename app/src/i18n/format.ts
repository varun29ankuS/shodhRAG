/**
 * Numbers, dates and plural forms in the user's locale: every call in the app goes through
 * these helpers, so none pins a locale of its own (`app/tests/i18nReady.test.ts`). The one
 * fixed format, an ISO date, is built from its parts, not from a locale that happens to
 * write dates that way.
 *
 * No imports: the Node tests load it.
 */

/** The locale used, `undefined` for the user's own (the system's). Tests set one. */
let active: string | undefined;

/** Use `locale` for every format (a test's), or the user's own again with none. */
export function setFormatLocale(locale?: string): void {
  active = locale;
}

/** The locale formats use now (`undefined`: the user's). */
export function formatLocale(): string | undefined {
  return active;
}

/** `n` grouped and written as the user's locale writes numbers: "1,204" in English. */
export function formatNumber(n: number, options?: Intl.NumberFormatOptions): string {
  return new Intl.NumberFormat(active, options).format(n);
}

/** A date or time as the user's locale writes it, with `options` choosing the parts. */
export function formatDate(value: Date | number, options?: Intl.DateTimeFormatOptions): string {
  return new Intl.DateTimeFormat(active, options).format(value);
}

/** Which plural form `n` takes in the user's locale: "one", "other", ... */
export function pluralCategory(n: number): Intl.LDMLPluralRule {
  return new Intl.PluralRules(active).select(n);
}

/**
 * The calendar day of `ms` as an ISO date (2026-11-26), in `timeZone` (the user's when
 * absent). Built from the date's parts, so no locale's way of writing dates is relied on.
 */
export function isoDate(ms: number, timeZone?: string): string {
  const parts = new Intl.DateTimeFormat('en-US', { year: 'numeric', month: '2-digit', day: '2-digit', timeZone, numberingSystem: 'latn' })
    .formatToParts(new Date(ms));
  const part = (type: Intl.DateTimeFormatPartTypes) => parts.find(p => p.type === type)?.value ?? '';
  return `${part('year')}-${part('month')}-${part('day')}`;
}
