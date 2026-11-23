// Pure helpers for the Insights page (kept separate so they can be unit-tested).

export interface DailyWords {
  date: string;
  words: number;
}

export function parseDay(date: string): Date {
  const [y, m, d] = date.split("-").map(Number);
  return new Date(y, (m ?? 1) - 1, d ?? 1);
}

export function isoDay(d: Date): string {
  const p = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}`;
}

function addDays(d: Date, n: number): Date {
  return new Date(d.getFullYear(), d.getMonth(), d.getDate() + n);
}

/** The Sunday that starts the week containing `d`. */
export function weekStart(d: Date): Date {
  return addDays(new Date(d.getFullYear(), d.getMonth(), d.getDate()), -d.getDay());
}

export interface WeekTotal {
  /** ISO date of the Sunday that starts the week */
  start: string;
  words: number;
}

/** Weekly totals (Sunday–Saturday) for the last `weeks` weeks, oldest first, ending with the week of `today`. */
export function weeklyTotals(daily: DailyWords[], weeks = 12, today = new Date()): WeekTotal[] {
  const thisWeek = weekStart(today);
  const first = addDays(thisWeek, -7 * (weeks - 1));
  const out: WeekTotal[] = Array.from({ length: weeks }, (_, i) => ({
    start: isoDay(addDays(first, 7 * i)),
    words: 0,
  }));
  for (const day of daily) {
    const ws = weekStart(parseDay(day.date));
    const idx = Math.round((ws.getTime() - first.getTime()) / (7 * 86_400_000));
    if (idx >= 0 && idx < weeks) out[idx].words += day.words;
  }
  return out;
}

/** 0 = none, 1 = 1–249, 2 = 250–499, 3 = 500–749, 4 = 750+ */
export function heatBand(words: number): 0 | 1 | 2 | 3 | 4 {
  if (!(words > 0)) return 0;
  if (words < 250) return 1;
  if (words < 500) return 2;
  if (words < 750) return 3;
  return 4;
}

export interface HeatCell {
  date: string;
  words: number;
  band: 0 | 1 | 2 | 3 | 4;
}

export interface Heatmap {
  /** columns = weeks (oldest first); each has 7 rows Sun..Sat; null = outside the range (future days) */
  columns: (HeatCell | null)[][];
  /** month label per column index (only on the first column of a month) */
  months: { col: number; label: string }[];
}

export function buildHeatmap(daily: DailyWords[], weeks = 26, today = new Date()): Heatmap {
  const byDate = new Map(daily.map((d) => [d.date, d.words]));
  const first = addDays(weekStart(today), -7 * (weeks - 1));
  const todayIso = isoDay(today);
  const columns: (HeatCell | null)[][] = [];
  const months: { col: number; label: string }[] = [];
  let lastMonth = -1;
  for (let c = 0; c < weeks; c++) {
    const col: (HeatCell | null)[] = [];
    for (let r = 0; r < 7; r++) {
      const d = addDays(first, c * 7 + r);
      const iso = isoDay(d);
      if (iso > todayIso) {
        col.push(null);
        continue;
      }
      const words = byDate.get(iso) ?? 0;
      col.push({ date: iso, words, band: heatBand(words) });
    }
    const sunday = addDays(first, c * 7);
    if (sunday.getMonth() !== lastMonth) {
      lastMonth = sunday.getMonth();
      months.push({ col: c, label: sunday.toLocaleDateString("en-GB", { month: "short" }) });
    }
    columns.push(col);
  }
  return { columns, months };
}

/** 200 -> "3 h 20 min"; 45 -> "45 min"; 0 -> "0 min" */
export function formatMinutes(minutes: number): string {
  const total = Math.max(0, Math.round(minutes));
  const h = Math.floor(total / 60);
  const m = total % 60;
  if (h === 0) return `${m} min`;
  if (m === 0) return `${h} h`;
  return `${h} h ${m} min`;
}

export interface SpeedStats {
  samples: number;
  transcribeWaitMedianMs: number;
  cleanupMedianMs: number;
  totalWaitMedianMs: number;
  fastModelShare: number;
}

/** "1.4 s" (one decimal; never shows 0.0 s). */
export function formatSeconds(ms: number): string {
  return `${Math.max(0.1, Math.round(ms / 100) / 10).toFixed(1)} s`;
}

/** Texts for the Speed card, or null when there are too few dictations to say anything. */
export function speedSummary(speed: SpeedStats | undefined | null): {
  total: string;
  transcribe: string;
  cleanup: string;
  fast: string;
} | null {
  if (!speed || speed.samples < 3) return null;
  const n = Math.round(speed.fastModelShare * speed.samples);
  return {
    total: formatSeconds(speed.totalWaitMedianMs),
    transcribe: formatSeconds(speed.transcribeWaitMedianMs),
    cleanup: formatSeconds(speed.cleanupMedianMs),
    fast: n === 0 ? "None used the fast model" : `${n} of ${speed.samples} used the fast model`,
  };
}
