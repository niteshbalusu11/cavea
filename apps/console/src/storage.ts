// Per-viewer preferences in local storage: versioned, and tolerant of
// storage that throws (a private window, blocked site data) or holds
// anything at all.

/** The stored value under `key`, parsed by `parse`, which returns
 * `undefined` for what it cannot use. */
export function load<T>(key: string, parse: (raw: unknown) => T | undefined, fallback: T): T {
  try {
    const text = localStorage.getItem(key);
    if (text === null) return fallback;
    return parse(JSON.parse(text)) ?? fallback;
  } catch {
    return fallback;
  }
}

export function save(key: string, value: unknown): void {
  try {
    localStorage.setItem(key, JSON.stringify(value));
  } catch {
    // Not stored: the preference lasts for this page only.
  }
}

export function isRecord(v: unknown): v is Record<string, unknown> {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}
