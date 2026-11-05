/**
 * Number and time formats shared by the host and plugins, so the session strip and every view
 * say the same thing the same way (compact in the strip, full digits in views).
 */

/** 1847302 → "1,847,302". */
export function formatDigits(n: number): string {
  const whole = Math.round(n);
  const sign = whole < 0 ? "-" : "";
  const digits = String(Math.abs(whole));
  let out = "";
  for (let i = 0; i < digits.length; i++) {
    if (i > 0 && (digits.length - i) % 3 === 0) out += ",";
    out += digits[i];
  }
  return sign + out;
}

/** 1847302 → "1.85M", 48210 → "48.2K", 140400 → "140K", 950 → "950". Three significant digits at most. */
export function formatCompact(n: number): string {
  const abs = Math.abs(n);
  const units: [number, string][] = [
    [1e9, "B"],
    [1e6, "M"],
    [1e3, "K"],
  ];
  let larger = 0;
  for (const [size, unit] of units) {
    if (abs >= size) {
      const value = n / size;
      const digits = Math.abs(value) >= 100 ? 0 : Math.abs(value) >= 10 ? 1 : 2;
      const text = value.toFixed(digits);
      // Rounding can carry into the next unit: 999,600 is "1M", not "1000K".
      if (larger > 0 && Math.abs(Number(text)) >= 1000) return formatCompact(Math.sign(n) * larger);
      // Only zeros after a decimal point go: "48.20" is "48.2", but "140" stays "140".
      return `${digits > 0 ? text.replace(/\.?0+$/, "") : text}${unit}`;
    }
    larger = size;
  }
  const whole = Math.round(n);
  if (Math.abs(whole) >= 1000) return formatCompact(Math.sign(n) * 1000);
  return String(whole);
}

/** 7_800_000 ms → "2h 10m"; 300_000 → "5m"; 42_000 → "42s". */
export function formatDuration(ms: number): string {
  const seconds = Math.max(0, Math.floor(ms / 1000));
  const hours = Math.floor(seconds / 3600);
  const minutes = Math.floor((seconds % 3600) / 60);
  if (hours > 0) return `${hours}h ${minutes}m`;
  if (minutes > 0) return `${minutes}m`;
  return `${seconds}s`;
}

/** Minutes to a level-up as players say it: "23 min", "2h 5m", "under a minute". */
export function formatMinutes(minutes: number): string {
  if (minutes < 1) return "under a minute";
  if (minutes < 60) return `${Math.round(minutes)} min`;
  const hours = Math.floor(minutes / 60);
  const rest = Math.round(minutes % 60);
  return rest === 0 ? `${hours}h` : `${hours}h ${rest}m`;
}

/** 6420 hundredths → "64.20%". */
export function formatPercent(hundredths: number): string {
  return `${Math.floor(hundredths / 100)}.${String(hundredths % 100).padStart(2, "0")}%`;
}
