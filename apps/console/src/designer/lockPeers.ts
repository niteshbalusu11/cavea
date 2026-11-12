// Two tabs of one person on one chart, in one browser, are one holder of
// its editing lock (D-73): every tab names the server the same editor, this
// browser's (`browserEditor`, D-96), so the server cannot tell them apart,
// and releasing it from one tab would free it under the other. Two
// browsers are two editors: the server keeps them apart itself, and one
// releases only its own lock. So a tab that is left releases the lock
// only when no other tab of this browser says it holds it. Tabs say so on
// a `BroadcastChannel` named by person, workspace and chart: "holding"
// when they hold it, "gone" when they stop or are left, and "hello" when
// they open, which the holders answer. A tab that dies without a word
// stays counted, so the last tab to leave then does not release, and the
// lock lapses within its minute (08 §6.2): never one freed under a tab.

/** What the page needs of a `BroadcastChannel`. */
export interface Channel {
  send(message: unknown): void;
  /** Called with every message another tab sends. */
  listen(heard: (data: unknown) => void): void;
  close(): void;
}

type Word = "hello" | "holding" | "gone";

export class LockPeers {
  readonly #channel: Channel;
  readonly #tab: string;
  #holding = false;
  /** Other tabs that said they hold the lock. */
  readonly #peers = new Set<string>();

  constructor(channel: Channel, tab: string) {
    this.#channel = channel;
    this.#tab = tab;
    channel.listen((data) => this.#heard(data));
    this.#say("hello");
  }

  /** Whether this tab holds the lock now. */
  set holding(holding: boolean) {
    if (holding === this.#holding) return;
    this.#holding = holding;
    this.#say(holding ? "holding" : "gone");
  }

  /** No other tab of this browser holds it: this one may release it. */
  get alone(): boolean {
    return this.#peers.size === 0;
  }

  close(): void {
    this.holding = false;
    this.#channel.close();
  }

  #say(word: Word): void {
    this.#channel.send({ tab: this.#tab, word });
  }

  #heard(data: unknown): void {
    if (typeof data !== "object" || data === null) return;
    const { tab, word } = data as { tab?: unknown; word?: unknown };
    if (typeof tab !== "string" || tab === this.#tab) return;
    if (word === "hello" && this.#holding) this.#say("holding");
    else if (word === "holding") this.#peers.add(tab);
    else if (word === "gone") this.#peers.delete(tab);
  }
}

/** The tabs of `user` on one chart, in this browser; a lone tab where
 * there is no `BroadcastChannel`. */
export function lockPeers(user: string, workspace: string, chart: string): LockPeers {
  const name = `cavea.designer.lock/${user}/${workspace}/${chart}`;
  const tab = crypto.randomUUID();
  if (typeof BroadcastChannel !== "function") {
    return new LockPeers({ send: () => {}, listen: () => {}, close: () => {} }, tab);
  }
  const bc = new BroadcastChannel(name);
  return new LockPeers(
    {
      // oxlint-disable-next-line unicorn/require-post-message-target-origin -- a BroadcastChannel takes no origin: it reaches this origin's tabs only
      send: (message) => bc.postMessage(message),
      listen: (heard) =>
        bc.addEventListener("message", (e: MessageEvent<unknown>) => heard(e.data)),
      close: () => bc.close(),
    },
    tab,
  );
}

const EDITOR_KEY = "cavea.designer.editor";
let editor: string | null = null;

/** This browser's editor id (D-96): one random id for every tab of this
 * browser profile, kept in its storage, so that the server tells this
 * browser's lock from another browser's of the same person. Where storage
 * is refused, the page's own (each such tab is then an editor apart). */
export function browserEditor(): string {
  if (editor !== null) return editor;
  try {
    const kept = localStorage.getItem(EDITOR_KEY);
    if (kept !== null && /^[A-Za-z0-9_-]{1,64}$/.test(kept)) return (editor = kept);
    const made = crypto.randomUUID();
    localStorage.setItem(EDITOR_KEY, made);
    return (editor = made);
  } catch {
    return (editor = crypto.randomUUID());
  }
}
