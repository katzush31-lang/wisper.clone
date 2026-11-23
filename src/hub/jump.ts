// Ranking for the "Jump to" search. Kept separate from the component so it can be unit-tested.
export interface JumpEntry {
  id: string;
  label: string;
  hint: string;
  keywords?: string;
}

/** Score how well `query` matches an entry; 0 means no match. Higher is better. */
export function jumpScore(e: JumpEntry, query: string): number {
  const q = query.trim().toLowerCase();
  if (!q) return 1;
  const label = e.label.toLowerCase();
  const words = q.split(/\s+/);
  const hay = `${label} ${e.hint.toLowerCase()} ${(e.keywords ?? "").toLowerCase()}`;
  if (!words.every((w) => hay.includes(w))) return 0;
  if (label === q) return 100;
  if (label.startsWith(q)) return 80;
  if (label.split(/[\s&]+/).some((w) => w.startsWith(words[0]))) return 60;
  if (label.includes(q)) return 40;
  return 20;
}

export function rankJump<T extends JumpEntry>(entries: T[], query: string): T[] {
  return entries
    .map((e, i) => ({ e, i, s: jumpScore(e, query) }))
    .filter((x) => x.s > 0)
    .sort((a, b) => b.s - a.s || a.i - b.i)
    .map((x) => x.e);
}
