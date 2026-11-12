// Publishing (07 §11, DSN-040): the draft is saved, then compiled and
// stored by the server as a new version (`POST …/publish` with `If-Match`,
// so what is published is what the person saw saved). The dialog shows the
// version published now and this draft's seats before, and the new version
// after; a refusal shows the server's findings. Not shown yet, for want of
// server endpoints: the diff against the previous version and the impact
// on events (SRV-036), and a thumbnail (no version holds one).
import { createSignal, For, Match, Show, Switch } from "solid-js";
import type { Chart, Finding } from "../api/generated.ts";
import { describe } from "../console/errors.ts";
import { formatNumber, msg } from "../i18n/messages.ts";
import { Button } from "../ui/controls.tsx";
import { Dialog } from "../ui/overlays.tsx";
import { holderName, type PublishOutcome } from "./ServerChart.tsx";

type Step =
  | { kind: "ready" }
  | { kind: "working" }
  | { kind: "done"; outcome: PublishOutcome; before: Chart["published_version"] };

export function PublishDialog(props: {
  open: boolean;
  name: string;
  chart: Chart | null;
  /** Seats in the draft as the editor has it. */
  seats: number;
  run: () => Promise<PublishOutcome>;
  onClose: () => void;
}) {
  const [step, setStep] = createSignal<Step>({ kind: "ready" });
  const current = () => props.chart?.published_version ?? null;

  const go = async () => {
    const before = current();
    setStep({ kind: "working" });
    setStep({ kind: "done", outcome: await props.run(), before });
  };
  const close = () => {
    if (step().kind === "working") return;
    setStep({ kind: "ready" });
    props.onClose();
  };
  const done = () => {
    const s = step();
    return s.kind === "done" ? s : null;
  };

  return (
    <Dialog
      open={props.open}
      title={msg("publish.title", { name: props.name })}
      heading
      onClose={close}
      class="max-w-lg"
    >
      <div class="space-y-3 px-5 pb-5 text-base">
        <Switch>
          <Match when={done()}>{(d) => <Result outcome={d().outcome} before={d().before} />}</Match>
          <Match when={true}>
            <p class="text-muted">{msg("publish.body")}</p>
            <ul class="space-y-1 text-fg">
              <li>
                <Show when={current()} fallback={msg("publish.none")}>
                  {(v) =>
                    msg("publish.current", {
                      version: v().number,
                      seats: formatNumber(v().stats.seats),
                    })
                  }
                </Show>
              </li>
              <li>{msg("publish.draft", { seats: formatNumber(props.seats) })}</li>
            </ul>
          </Match>
        </Switch>
        <div class="flex justify-end gap-2 pt-2">
          <Show
            when={step().kind !== "done"}
            fallback={
              <Button variant="primary" onClick={close}>
                {msg("action.close")}
              </Button>
            }
          >
            <Button onClick={close} disabled={step().kind === "working"}>
              {msg("action.cancel")}
            </Button>
            <Button
              variant="primary"
              disabled={step().kind === "working"}
              onClick={() => void go()}
            >
              {step().kind === "working" ? msg("publish.working") : msg("publish.action")}
            </Button>
          </Show>
        </div>
      </div>
    </Dialog>
  );
}

function Result(props: { outcome: PublishOutcome; before: Chart["published_version"] }) {
  const o = props.outcome;
  switch (o.kind) {
    case "published": {
      const v = o.version;
      if (!o.created) {
        return <p role="status">{msg("publish.unchanged", { version: v.number })}</p>;
      }
      const before = props.before;
      const delta = before === null ? 0 : v.stats.seats - before.stats.seats;
      return (
        <div role="status" class="space-y-1">
          <p class="font-medium text-fg">
            {msg("publish.done", { version: v.number, seats: formatNumber(v.stats.seats) })}
          </p>
          <Show when={before !== null && delta !== 0}>
            <p class="text-muted">
              {msg("publish.delta", {
                delta: `${delta > 0 ? "+" : "−"}${formatNumber(Math.abs(delta))}`,
                version: before?.number ?? 0,
              })}
            </p>
          </Show>
        </div>
      );
    }
    case "invalid":
      return <Findings findings={o.findings} />;
    case "notSaved":
      return <p role="alert">{msg("publish.notSaved")}</p>;
    case "conflict":
      return <p role="alert">{msg("publish.conflict")}</p>;
    case "locked":
      return <p role="alert">{msg("publish.locked", { who: holderName(o.lock, "") })}</p>;
    case "failed":
      return <p role="alert">{msg("publish.failed", { message: describe(o.error) })}</p>;
  }
}

function Findings(props: { findings: Finding[] }) {
  const errors = () => props.findings.filter((f) => f.severity === "error");
  return (
    <div role="alert">
      <p class="text-fg">{msg("publish.invalid")}</p>
      <ul class="mt-2 max-h-60 space-y-1 overflow-y-auto text-sm">
        <For each={errors()}>
          {(f) => (
            <li class="text-danger">
              <span class="font-mono">{f.code}</span> {f.message}
            </li>
          )}
        </For>
      </ul>
    </div>
  );
}
