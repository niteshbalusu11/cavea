// Optimistic updates with rollback (09 §8 "Feedback"), for the changes that
// are safe to show before the server agrees: a role, an archive, a name.
// One value at a time: a change is shown at once; the server's answer
// replaces it; a refusal puts back what the server last said. Several
// changes to the same value in flight: what is shown is the newest one
// still on its way, and once all have answered, the screen reads the value
// again (`onRaced`), since answers can arrive in another order than the
// server applied them.

export class Optimistic<T> {
  #confirmed: T;
  #confirmedAt = 0;
  #next = 0;
  readonly #pending = new Map<number, T>();
  #raced = false;

  constructor(
    value: T,
    private readonly show: (value: T) => void,
    private readonly onRaced: () => void = () => {},
  ) {
    this.#confirmed = value;
  }

  /** What is on screen: the newest change on its way, unless the server
   * has answered a newer one; else what the server said. */
  get shown(): T {
    let newest = -1;
    for (const id of this.#pending.keys()) newest = Math.max(newest, id);
    const value = this.#pending.get(newest);
    return newest > this.#confirmedAt && value !== undefined ? value : this.#confirmed;
  }

  get busy(): boolean {
    return this.#pending.size > 0;
  }

  /** Shows `value` at once; returns the change's id for its answer. */
  begin(value: T): number {
    const id = ++this.#next;
    this.#pending.set(id, value);
    if (this.#pending.size > 1) this.#raced = true;
    this.show(this.shown);
    return id;
  }

  /** The server agreed, and holds `value` now. */
  confirm(id: number, value: T): void {
    if (!this.#pending.delete(id)) return;
    // An older change's answer never overrides a newer one's.
    if (id > this.#confirmedAt) {
      this.#confirmed = value;
      this.#confirmedAt = id;
    }
    this.#settle();
  }

  /** The server refused: the change is taken back. Returns what is shown
   * now, for the message saying so. */
  fail(id: number): T {
    if (this.#pending.delete(id)) this.#settle();
    return this.shown;
  }

  #settle(): void {
    this.show(this.shown);
    if (this.#pending.size === 0 && this.#raced) {
      this.#raced = false;
      this.onRaced();
    }
  }
}

/** One `Optimistic` per row of a list, by its key, made on first use. */
export function optimisticRows<T>(
  show: (key: string, value: T) => void,
  onRaced: () => void,
): (key: string, current: T) => Optimistic<T> {
  const rows = new Map<string, Optimistic<T>>();
  return (key, current) => {
    let row = rows.get(key);
    // A row with nothing on its way starts from what the list holds now
    // (it may have been read again since).
    if (row === undefined || !row.busy) {
      row = new Optimistic(current, (v) => show(key, v), onRaced);
      rows.set(key, row);
    }
    return row;
  };
}
