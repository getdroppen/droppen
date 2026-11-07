/** One language's column of a copy table, keyed by its English line. */
export function columnOf(
  rows: readonly (readonly [string, string, string, string])[],
  index: 1 | 2 | 3,
): Record<string, string> {
  return Object.fromEntries(rows.map((row) => [row[0], row[index]]));
}
