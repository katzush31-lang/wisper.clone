// Small pure helpers shared by the Dictionary, Snippets, Style, Transforms and Scratchpad pages.

/** "WhatsApp to family!" -> "whatsapp_to_family" */
export function slugify(name: string): string {
  const s = name
    .toLowerCase()
    .normalize("NFKD")
    .replace(/[̀-ͯ]/g, "")
    .replace(/[^a-z0-9]+/g, "_")
    .replace(/^_+|_+$/g, "");
  return s || "untitled";
}

/** prefix + slug of name, made unique against `existing` by adding _2, _3 … */
export function uniqueId(prefix: string, name: string, existing: Iterable<string>): string {
  const taken = new Set(existing);
  const base = prefix + slugify(name);
  if (!taken.has(base)) return base;
  let n = 2;
  while (taken.has(`${base}_${n}`)) n++;
  return `${base}_${n}`;
}

/** "a, b ,, c" -> ["a","b","c"] (also accepts new lines), removing duplicates. */
export function splitList(text: string): string[] {
  const out: string[] = [];
  for (const part of text.split(/[,\n]/)) {
    const t = part.trim();
    if (t && !out.some((o) => o.toLowerCase() === t.toLowerCase())) out.push(t);
  }
  return out;
}

export function joinList(list: string[]): string {
  return list.join(", ");
}

export interface CsvPreview {
  words: number;
  corrections: number;
}

/** Counts what a pasted dictionary CSV contains: plain words vs "misspelling,correction" lines. */
export function previewDictionaryCsv(text: string): CsvPreview {
  let words = 0;
  let corrections = 0;
  for (const line of text.split(/\r?\n/)) {
    const t = line.trim();
    if (!t) continue;
    const parts = t.split(",").map((p) => p.trim());
    if (parts.length >= 2 && parts[0] && parts[1]) corrections++;
    else if (parts[0]) words++;
  }
  return { words, corrections };
}

/** How many snippets a pasted JSON export holds; null if it isn't a valid list. */
export function previewSnippetsJson(text: string): number | null {
  if (!text.trim()) return null;
  try {
    const data: unknown = JSON.parse(text);
    if (!Array.isArray(data)) return null;
    return data.filter((x) => {
      if (!x || typeof x !== "object") return false;
      const o = x as Record<string, unknown>;
      return (
        (typeof o.name === "string" && typeof o.text === "string") ||
        (typeof o.trigger === "string" && typeof o.expansion === "string")
      );
    }).length;
  } catch {
    return null;
  }
}

/** First three words of the body, used as the note title when the title is empty. */
export function autoTitle(body: string): string {
  const words = body.trim().split(/\s+/).filter(Boolean).slice(0, 3);
  const t = words.join(" ").replace(/[.,;:!?-]+$/, "");
  return t.length > 40 ? t.slice(0, 40).trim() : t;
}

/** Inserts `text` at the caret (or replaces the selection). Appends if the caret is unknown. */
export function insertAtCaret(
  body: string,
  text: string,
  start: number | null,
  end: number | null,
): { value: string; caret: number } {
  if (start === null || end === null || start > body.length) {
    const sep = body && !/\s$/.test(body) ? " " : "";
    const value = body + sep + text;
    return { value, caret: value.length };
  }
  const value = body.slice(0, start) + text + body.slice(end);
  return { value, caret: start + text.length };
}

/** "just now", "5 min ago", "3 h ago", "Yesterday", "12 Nov" */
export function relativeTime(ms: number, now = Date.now()): string {
  const diff = Math.max(0, now - ms);
  const min = Math.floor(diff / 60_000);
  if (min < 1) return "Just now";
  if (min < 60) return `${min} min ago`;
  const h = Math.floor(min / 60);
  if (h < 24) return `${h} h ago`;
  const days = Math.floor(h / 24);
  if (days === 1) return "Yesterday";
  if (days < 7) return `${days} days ago`;
  return new Date(ms).toLocaleDateString("en-GB", { day: "numeric", month: "short" });
}

export function truncate(text: string, max: number): string {
  const one = text.replace(/\s+/g, " ").trim();
  return one.length > max ? one.slice(0, max - 1).trimEnd() + "…" : one;
}
