// Pure helpers for the live waveform on the floating bar.

/** Push a new level (0..1) onto a fixed-length history, newest last. */
export function pushLevel(history: number[], level: number, size: number): number[] {
  const v = Math.min(1, Math.max(0, Number.isFinite(level) ? level : 0));
  const next = [...history, v];
  while (next.length < size) next.unshift(0);
  return next.slice(next.length - size);
}

/** Convert a level to a bar height in px. Speech levels are small, so apply a curve. */
export function barHeight(level: number, min = 4, max = 22): number {
  const v = Math.min(1, Math.max(0, level));
  const curved = Math.pow(v, 0.55);
  return Math.round(min + (max - min) * curved);
}
