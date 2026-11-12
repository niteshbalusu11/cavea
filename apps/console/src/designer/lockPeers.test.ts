// The two-tabs rule (D-73): a tab left releases the lock only when no other
// tab of the same person holds it.
import { expect, test } from "bun:test";
import { LockPeers, type Channel } from "./lockPeers.ts";

/** One browser's channels of one name: a message reaches every other. */
function bus() {
  const channels = new Map<Channel, (data: unknown) => void>();
  return (): Channel => {
    const c: Channel = {
      send(message) {
        for (const [other, heard] of channels) if (other !== c) heard(message);
      },
      listen: (heard) => void channels.set(c, heard),
      close: () => void channels.delete(c),
    };
    return c;
  };
}

test("closing one of two tabs holding the lock does not release it; closing the last does", () => {
  const open = bus();
  const a = new LockPeers(open(), "a");
  a.holding = true;
  expect(a.alone).toBe(true);
  // B opens later and takes (renews) the same lock: each counts the other.
  const b = new LockPeers(open(), "b");
  expect(b.alone).toBe(false);
  b.holding = true;
  expect(a.alone).toBe(false);
  // A is closed: not alone, so it does not release; B is alone now.
  expect(a.alone).toBe(false);
  a.close();
  expect(b.alone).toBe(true);
});

test("a tab that is read-only (another person holds the lock) does not count", () => {
  const open = bus();
  const reading = new LockPeers(open(), "r");
  const editing = new LockPeers(open(), "e");
  editing.holding = true;
  expect(editing.alone).toBe(true);
  // Taken over in the editing tab: it stops holding, and says so.
  reading.holding = true;
  reading.holding = false;
  expect(editing.alone).toBe(true);
});

test("a tab that dies without a word keeps the others from releasing: the lock lapses instead", () => {
  const open = bus();
  const a = new LockPeers(open(), "a");
  const b = new LockPeers(open(), "b");
  a.holding = true;
  b.holding = true;
  // B's process is killed: no "gone".
  expect(a.alone).toBe(false);
});
