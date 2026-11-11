// The command palette's fuzzy match (07 §8): every character of the query
// in order, case and accents ignored. Matches are ranked by how they read:
// the whole query at the start of a word beats it inside a word, which
// beats scattered letters; letters at word starts and in runs score more.

export interface Match {
  score: number;
  /** Indices of the matched characters in the text, for highlighting. */
  at: number[];
}

const fold = (s: string) => s.normalize("NFD").replace(/\p{M}/gu, "").toLowerCase();

const isStart = (text: string, i: number) => i === 0 || /[\s\-_/.(]/.test(text[i - 1] ?? " ");

/** How well `query` matches `text`, or `null`; an empty query matches all. */
export function match(query: string, text: string): Match | null {
  const q = fold(query).replace(/\s+/g, " ").trim();
  if (q === "") return { score: 0, at: [] };
  const t = fold(text);
  // Folding can change the length (ligatures); positions are then only
  // approximate for highlighting, never for the score.
  const exact = t.indexOf(q);
  if (exact >= 0) {
    const at = Array.from({ length: q.length }, (_, k) => exact + k);
    return { score: (isStart(t, exact) ? 3000 : 2000) - exact, at };
  }
  // Scattered: each query letter at its earliest place after the last.
  const at: number[] = [];
  let score = 1000;
  let from = 0;
  for (const ch of q) {
    if (ch === " ") continue;
    const i = t.indexOf(ch, from);
    if (i < 0) return null;
    if (isStart(t, i)) score += 10;
    if (at.at(-1) === i - 1) score += 5;
    score -= i - from;
    at.push(i);
    from = i + 1;
  }
  return { score, at };
}

/** The items matching `query`, best first; ties keep the items' order. */
export function rank<T>(items: readonly T[], query: string, text: (item: T) => string) {
  const out: { item: T; match: Match }[] = [];
  for (const item of items) {
    const m = match(query, text(item));
    if (m !== null) out.push({ item, match: m });
  }
  // Array.prototype.sort is stable: equal scores keep the given order.
  // oxlint-disable-next-line unicorn/no-array-sort -- `out` is ours; `toSorted` is ES2023, past the lib (ES2022).
  return out.sort((a, b) => b.match.score - a.match.score);
}
