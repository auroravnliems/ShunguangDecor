// Small formatting helpers for the UI.

/** Bytes → rounded MB count; "—" when there is no data. */
export function formatMb(bytes: number | null): string {
  if (bytes === null || !Number.isFinite(bytes)) return "—";
  return String(Math.round(bytes / (1024 * 1024)));
}

/** "yyyyMMdd-HHmm" in local time, used in suggested file names. */
export function stamp(d: Date): string {
  const p = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}${p(d.getMonth() + 1)}${p(d.getDate())}-${p(d.getHours())}${p(d.getMinutes())}`;
}
