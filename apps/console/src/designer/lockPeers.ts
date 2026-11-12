// Two tabs of one person on one chart are one holder of its editing lock
// (D-73): the server cannot tell them apart, and releasing it from one tab
// would free it under the other. So a tab that is left releases the lock
// only when no other tab of this browser says it holds it. Tabs say so on
// a `BroadcastChannel` named by person, workspace and chart: "holding"
// when they hold it, "gone" when they stop or are left, and "hello" when
// they open, which the holders answer. A tab that dies without a word
// stays counted, so the last tab to leave then does not release, and the
// lock lapses within its minute (08 §6.2): never one freed under a tab.

/** What the page needs of a `BroadcastChannel`. */
export interface Channel {
  postMessage(message: unknown): void;
  onmessage: ((event: { data: unknown }) => void) | null;
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
    channel.onmessage = (event) => this.#heard(event.data);
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
    this.#channel.onmessage = null;
    this.#channel.close();
  }

  #say(word: Word): void {
    this.#channel.postMessage({ tab: this.#tab, word });
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
  const channel: Channel =
    typeof BroadcastChannel === "function"
      ? new BroadcastChannel(name)
      : { postMessage: () => {}, onmessage: null, close: () => {} };
  return new LockPeers(channel, tab);
}
