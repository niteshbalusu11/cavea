// A countdown for a refusal that says when to try again (`Retry-After`).
import { createSignal, onCleanup } from "solid-js";

export function createWait() {
  const [seconds, setSeconds] = createSignal(0);
  let timer: ReturnType<typeof setInterval> | undefined;
  const stop = () => {
    if (timer !== undefined) clearInterval(timer);
    timer = undefined;
  };
  onCleanup(stop);
  return {
    seconds,
    start(total: number) {
      stop();
      const until = Date.now() + total * 1000;
      setSeconds(total);
      timer = setInterval(() => {
        const left = Math.ceil((until - Date.now()) / 1000);
        setSeconds(Math.max(0, left));
        if (left <= 0) stop();
      }, 250);
    },
  };
}
