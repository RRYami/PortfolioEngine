// Number / date formatting. Negative numbers use the Unicode minus
// "−" (U+2212), not a hyphen — matching the design tokens.

export const MINUS = "−";

/** Locale grouped number with a fixed number of decimals. */
export function nf(n: number, d = 0): string {
  return Number(n).toLocaleString("en-US", {
    minimumFractionDigits: d,
    maximumFractionDigits: d,
  });
}

/** Compact money: `$1.85M`, `$278.1k`, `$600`. Negatives prefixed with "−". */
export function comp(n: number): string {
  const a = Math.abs(n);
  let s: string;
  if (a >= 1e6) s = "$" + (a / 1e6).toFixed(2) + "M";
  else if (a >= 1e3) s = "$" + (a / 1e3).toFixed(1) + "k";
  else s = "$" + a.toFixed(0);
  return (n < 0 ? MINUS : "") + s;
}

const MONTHS = [
  "Jan", "Feb", "Mar", "Apr", "May", "Jun",
  "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/** "2025-10-13" → "Oct 13" (parsed from the string, timezone-agnostic). */
export function shortDate(iso: string): string {
  const [, m, d] = iso.split("-").map(Number);
  return `${MONTHS[m - 1]} ${d}`;
}

/** "2026-06-25T16:00:00-04:00" → "As of Jun 25, 2026 · 16:00 ET". */
export function asOfLabel(iso: string): string {
  const [date, time = ""] = iso.split("T");
  const [y, m, d] = date.split("-").map(Number);
  const hhmm = time.slice(0, 5);
  return `As of ${MONTHS[m - 1]} ${d}, ${y} · ${hhmm} ET`;
}
