/**
 * Counts with their noun in the right number: "1 source", "2 sources",
 * "1,204 files". Every count shown in the UI goes through here, so none
 * reads "1 sources".
 *
 * Pure module so it is unit-tested with Node (`app/tests/plural.test.ts`).
 */

/** The noun for `n` things: `one` for exactly 1, else `many` (default `one` + "s"). */
export function nounFor(n: number, one: string, many = `${one}s`): string {
  return n === 1 ? one : many;
}

/** `n` (grouped as the user's locale groups numbers) and its noun: "1 source", "1,204 files", "0 passages". */
export function countOf(n: number, one: string, many = `${one}s`): string {
  return `${n.toLocaleString()} ${nounFor(n, one, many)}`;
}
