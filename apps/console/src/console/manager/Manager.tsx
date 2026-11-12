// The event manager, basic (09 §4.2, CON-008; D-68): the seat map, live,
// beside an operations panel. The map is the buyer's renderer in picker
// mode fed by `createLive`: free seats are drawn available, other seats
// unavailable, and a click on an unavailable seat selects it all the same.
// The panel says the true status of each selected seat, acts on the
// selection (block, unblock, book, release; all or nothing), shows which
// seats stopped a refused action, and the details of the seat last clicked.
import {
  createSeatMap,
  type CaveaError,
  type Metrics,
  type SeatMap,
  type SeatObject,
  type Selection,
} from "@cavea/renderer";
import { createEffect, createMemo, createSignal, For, onMount, Show } from "solid-js";
import { checkEvent } from "../../api/check.ts";
import { call, type ApiError } from "../../api/client.ts";
import type { Event } from "../../api/generated.ts";
import { allows } from "../../api/permissions.ts";
import { asApiError, componentSignal } from "../../api/query.ts";
import { msg, msgOf } from "../../i18n/messages.ts";
import { loadSession, workspace } from "../../session.ts";
import { resolvedTheme } from "../../theme.ts";
import { Button, IconButton } from "../../ui/controls.tsx";
import { Field, length } from "../../ui/form.tsx";
import { Confirm, Dialog, notify } from "../../ui/overlays.tsx";
import { describe, reference, report } from "../errors.ts";
import { roleName } from "../Events.tsx";
import { Action } from "../kit.tsx";
import { createLive, type Outcome } from "./live.ts";
import { blockers, namedSeats, type Action as SeatAction, type Status } from "./model.ts";

/** The picker's largest selection (05 §3). */
const MAX = 100;

const time = new Intl.DateTimeFormat(undefined, { timeStyle: "medium" });
const dateTime = new Intl.DateTimeFormat(undefined, { dateStyle: "medium", timeStyle: "medium" });

export function statusName(status: Status): string {
  return typeof status === "number"
    ? msg("manager.status.custom", { code: status })
    : msg(`manager.status.${status}`);
}

const seatsText = (labels: readonly string[]) => {
  const { named, more } = namedSeats(labels);
  return more === 0 ? msg("manager.seats", { named }) : msg("manager.seatsMore", { named, more });
};

const reasonText = (reason: string) =>
  msgOf("manager.reason", reason) ?? msg("manager.reason.other", { reason });

const canAct = () => allows(workspace(), "changeStatus");

interface Picked {
  label: string;
  /** Its status when it was selected, to say when it changed since. */
  was: Status;
}

interface Refused {
  action: SeatAction;
  error: ApiError;
  refused: { label: string; reason: string }[];
}

export function Manager(props: { event: Event; onEvent: (event: Event) => void }) {
  const signal = componentSignal();
  const live = createLive({
    event: props.event.key,
    chartUrl: props.event.version.url,
    signal,
    onCounts: (counts, seq) => props.onEvent({ ...props.event, counts, seq }),
  });
  const [map, setMap] = createSignal<SeatMap | null>(null);
  const [failed, setFailed] = createSignal<CaveaError | ApiError | null>(null);
  const [picked, setPicked] = createSignal<Picked[]>([]);
  const [inspected, setInspected] = createSignal<SeatObject | null>(null);
  const [refusal, setRefusal] = createSignal<Refused | null>(null);
  const [busy, setBusy] = createSignal(false);
  const [booking, setBooking] = createSignal(false);
  const [releasing, setReleasing] = createSignal(false);
  let host: HTMLDivElement | undefined;

  const labels = () => picked().map((p) => p.label);
  const seats = createMemo(() => {
    live.version();
    return picked().map((p) => ({ ...p, status: live.statusOf(p.label) }));
  });

  const inspect = (o: SeatObject | null, read: boolean) => {
    setInspected(o);
    live.inspect(o?.label ?? null);
    if (o !== null && read) void live.read([o.label]).catch(() => {});
  };

  const onSelection = (selection: Selection) => {
    const now = selection.objects.map((o) => o.label);
    live.setSelected(now);
    const before = new Map(picked().map((p) => [p.label, p]));
    setPicked(now.map((label) => before.get(label) ?? { label, was: live.statusOf(label) }));
    if (now.length === 0) setRefusal(null);
  };

  /** A click on a seat the map draws unavailable: selected all the same. */
  const takeAnyway = (objects: SeatObject[]) => {
    const wanted = objects.map((o) => o.label);
    const last = objects.at(-1);
    if (last !== undefined) inspect(last, true);
    live.allow(wanted);
    // After the map has finished delivering the refusal.
    queueMicrotask(() => {
      map()?.select(wanted);
      live.settle(wanted);
    });
  };

  async function start(): Promise<void> {
    if (host === undefined) return;
    setFailed(null);
    try {
      // The chart comes from the adapter, which reads its labels too.
      const created = await createSeatMap(host, {
        state: live.adapter,
        selection: { max: MAX },
        theme: resolvedTheme(),
        signal,
        ui: {
          toasts: false,
          tooltip: (o) =>
            msg("manager.tooltip", { label: o.label, status: statusName(live.statusOf(o.label)) }),
        },
        messages: {
          available: msg("manager.legendFree"),
          unavailable: msg("manager.legendTaken"),
          selected: msg("manager.legendSelected"),
          soldOut: msg("manager.noneFree"),
        },
        onSelectionChange: onSelection,
        onObjectSelect: (o) => inspect(o, true),
        onObjectDeselect: (o) => inspect(o, true),
        onObjectFocus: (o) => {
          if (o !== null) inspect(o, false);
        },
        onSelectionInvalid: (rule, objects) => {
          if (rule === "not_free") takeAnyway(objects);
          else if (rule === "max") notify(msg("manager.max", { n: MAX }), "error");
        },
        onError: (e) => notify(msg("manager.mapError", { code: e.code }), "error"),
        // Frame statistics, for measuring in a dev build's console.
        ...(import.meta.env.DEV
          ? {
              onMetrics: (m: Metrics) => {
                const w = window as unknown as { caveaManager?: { metrics?: Metrics[] } };
                w.caveaManager?.metrics?.push(m);
              },
            }
          : {}),
      });
      setMap(created);
      if (import.meta.env.DEV) {
        (window as unknown as { caveaManager?: unknown }).caveaManager = {
          map: created,
          live,
          metrics: [],
        };
      }
    } catch (e) {
      if (signal.aborted) return;
      setFailed(e instanceof Error && "code" in e ? (e as CaveaError) : asApiError(e));
    }
  }
  onMount(() => void start());
  createEffect(() => map()?.setTheme(resolvedTheme()));

  const archived = () => props.event.archived_at !== null;
  const blocked = (action: SeatAction): string | null => {
    if (archived()) return msg("manager.archived");
    if (busy()) return msg("manager.busy");
    if (seats().length === 0) return msg("manager.needSelection");
    const n = blockers(action, seats()).length;
    if (n === 0) return null;
    const id =
      action === "unblock"
        ? "manager.notBlocked"
        : action === "release"
          ? "manager.notBooked"
          : "manager.notFree";
    return msg(id, { n });
  };

  async function act(action: SeatAction, orderRef?: string): Promise<Outcome | null> {
    const targets = labels();
    if (targets.length === 0 || busy()) return null;
    setBusy(true);
    setRefusal(null);
    try {
      const outcome = await live.act(action, targets, orderRef);
      if (outcome.ok) {
        notify(msg(`manager.done.${action}`, { n: outcome.labels.length }));
        map()?.clearSelection();
      } else {
        setRefusal({ action, error: outcome.error, refused: outcome.refused });
        // The panel says it; a role lowered since is read again, and the
        // event (archived since?) quietly.
        if (outcome.error.code === "forbidden") void loadSession();
        void call("getEvent", { path: { event: props.event.key } }, signal)
          .then((e) => props.onEvent(checkEvent(e)))
          .catch(() => {});
      }
      return outcome;
    } catch (e) {
      report(e);
      return null;
    } finally {
      setBusy(false);
    }
  }

  return (
    <div class="flex min-h-0 flex-1">
      <div class="relative min-w-0 flex-1 bg-canvas">
        <div
          ref={(el) => (host = el)}
          class="absolute inset-0"
          role="region"
          aria-label={msg("manager.map", { name: props.event.name ?? props.event.key })}
        />
        <Show when={map() === null && failed() === null}>
          <p role="status" class="absolute top-4 left-4 text-base text-muted">
            {msg("manager.mapLoading")}
          </p>
        </Show>
        <Show when={failed()}>
          {(e) => (
            <div
              role="alert"
              class="absolute top-4 left-4 max-w-md rounded-lg bg-surface p-4 shadow-menu"
            >
              <p class="text-base text-fg">{msg("manager.mapFailed")}</p>
              <p class="mt-1 text-sm text-muted">
                {"status" in e() ? describe(e() as ApiError) : e().message}
              </p>
              <Button class="mt-3 h-8" onClick={() => void start()}>
                {msg("action.retry")}
              </Button>
            </div>
          )}
        </Show>
      </div>
      <aside class="w-80 shrink-0 overflow-y-auto border-l border-line bg-surface">
        <Health live={live} />
        <section class="border-b border-line p-4" aria-labelledby="manager-selection">
          <div class="flex items-center gap-2">
            <h2 id="manager-selection" class="mr-auto text-base font-semibold text-fg">
              {seats().length === 0
                ? msg("manager.selection")
                : msg("manager.selectionCount", { n: seats().length })}
            </h2>
            <Show when={seats().length > 0}>
              <Button class="h-7" onClick={() => map()?.clearSelection()}>
                {msg("manager.clear")}
              </Button>
            </Show>
          </div>
          <Show
            when={seats().length > 0}
            fallback={<p class="mt-2 text-sm text-muted">{msg("manager.selectionEmpty")}</p>}
          >
            <ul class="mt-2 max-h-64 divide-y divide-line overflow-y-auto rounded-md border border-line">
              <For each={seats()}>
                {(s) => (
                  <li class="flex items-center gap-2 px-2 py-1">
                    <span class="min-w-0 flex-1 truncate font-mono text-sm text-fg">{s.label}</span>
                    <span class="text-sm text-muted">{statusName(s.status)}</span>
                    <Show when={s.status !== s.was}>
                      <span class="rounded-sm bg-warning-soft px-1 text-xs text-warning">
                        {msg("manager.was", { status: statusName(s.was) })}
                      </span>
                    </Show>
                    <IconButton
                      icon="close"
                      label={msg("manager.deselect", { label: s.label })}
                      onClick={() => map()?.deselect([s.label])}
                    />
                  </li>
                )}
              </For>
            </ul>
          </Show>
          <Show
            when={canAct()}
            fallback={
              <p class="mt-3 text-sm text-muted">{msg("manager.roleView", { role: roleName() })}</p>
            }
          >
            <h3 class="sr-only">{msg("manager.actions")}</h3>
            <div class="mt-3 grid grid-cols-2 gap-2">
              <Action
                label={msg("manager.block")}
                blocked={blocked("block")}
                onClick={() => void act("block")}
              />
              <Action
                label={msg("manager.unblock")}
                blocked={blocked("unblock")}
                onClick={() => void act("unblock")}
              />
              <Action
                label={msg("manager.book")}
                variant="primary"
                blocked={blocked("book")}
                onClick={() => setBooking(true)}
              />
              <Action
                label={msg("manager.release")}
                variant="danger"
                blocked={blocked("release")}
                onClick={() => setReleasing(true)}
              />
            </div>
            <Show when={seats().some((s) => s.status === "held")}>
              <p class="mt-2 text-sm text-muted">{msg("manager.heldNote")}</p>
            </Show>
          </Show>
          <Show when={refusal()}>{(r) => <RefusalNote refusal={r()} live={live} />}</Show>
        </section>
        <Details seat={inspected()} live={live} />
        <section class="p-4">
          <h2 class="text-base font-semibold text-fg">{msg("manager.howTitle")}</h2>
          <p class="mt-1 text-sm text-muted">{msg("manager.how")}</p>
        </section>
      </aside>
      <BookDialog
        open={booking()}
        labels={labels()}
        onClose={() => setBooking(false)}
        onBook={async (ref) => {
          const outcome = await act("book", ref);
          setBooking(false);
          return outcome;
        }}
      />
      <Confirm
        open={releasing()}
        title={msg("manager.releaseTitle", { n: labels().length })}
        description={msg("manager.releaseBody", { seats: seatsText(labels()) })}
        confirm={msg("manager.releaseConfirm")}
        variant="danger"
        onConfirm={() => void act("release")}
        onClose={() => setReleasing(false)}
      />
    </div>
  );
}

/** When the seats were read, and whether what is shown may be stale. */
function Health(props: { live: ReturnType<typeof createLive> }) {
  const h = () => props.live.health();
  return (
    <p
      class="border-b border-line px-4 py-2 text-sm"
      classList={{
        "bg-warning-soft text-warning": h().error !== null,
        "text-muted": h().error === null,
      }}
      role="status"
    >
      <Show
        when={h().error}
        fallback={
          <Show when={h().checkedAt} fallback={msg("manager.liveStarting")}>
            {(at) => <>{msg("manager.liveEvery", { time: time.format(at()) })}</>}
          </Show>
        }
      >
        {(e) => (
          <>
            {h().checkedAt === null
              ? msg("manager.staleNever", { problem: describe(e()) })
              : msg("manager.stale", {
                  problem: describe(e()),
                  time: time.format(h().checkedAt ?? 0),
                })}
          </>
        )}
      </Show>
    </p>
  );
}

function RefusalNote(props: { refusal: Refused; live: ReturnType<typeof createLive> }) {
  const unknown = () =>
    props.refusal.error.code === "network" || props.refusal.error.code === "timeout";
  return (
    <div role="alert" class="mt-3 rounded-md border border-danger p-3">
      <p class="text-sm font-medium text-danger">{msg("manager.refusedTitle")}</p>
      <p class="mt-1 text-sm text-fg">
        {unknown() ? msg("manager.unknownOutcome") : describe(props.refusal.error)}
      </p>
      <Show when={props.refusal.refused.length > 0}>
        <p class="mt-2 text-sm text-muted">{msg("manager.refusedSeats")}</p>
        <ul class="mt-1 space-y-0.5 text-sm text-fg">
          <For each={props.refusal.refused}>
            {(r) => (
              <li>
                {
                  (props.live.version(),
                  msg("manager.refusedNow", {
                    label: r.label,
                    reason: reasonText(r.reason),
                    status: statusName(props.live.statusOf(r.label)),
                  }))
                }
              </li>
            )}
          </For>
        </ul>
      </Show>
      <Show when={reference(props.refusal.error)}>
        {(ref) => <p class="mt-1 text-xs text-faint">{ref()}</p>}
      </Show>
    </div>
  );
}

function Row(props: { term: string; value: string }) {
  return (
    <div class="flex gap-2">
      <dt class="w-28 shrink-0 text-muted">{props.term}</dt>
      <dd class="min-w-0 break-words text-fg">{props.value}</dd>
    </div>
  );
}

function Details(props: { seat: SeatObject | null; live: ReturnType<typeof createLive> }) {
  const object = () => {
    props.live.version();
    return props.seat === null ? undefined : props.live.object(props.seat.label);
  };
  const status = () => {
    props.live.version();
    return props.seat === null ? "free" : props.live.statusOf(props.seat.label);
  };
  return (
    <section class="border-b border-line p-4" aria-labelledby="manager-details">
      <h2 id="manager-details" class="text-base font-semibold text-fg">
        {msg("manager.details")}
      </h2>
      <Show
        when={props.seat}
        fallback={<p class="mt-2 text-sm text-muted">{msg("manager.detailsEmpty")}</p>}
      >
        {(seat) => (
          <div aria-live="polite">
            <p class="mt-2 font-mono text-base text-fg">{seat().label}</p>
            <Show when={seat().row !== undefined}>
              <p class="text-sm text-muted">
                {msg("manager.place", {
                  section: seat().section ?? msg("manager.none"),
                  row: seat().row ?? msg("manager.none"),
                  seat: seat().seat ?? msg("manager.none"),
                })}
              </p>
            </Show>
            <dl class="mt-2 space-y-1 text-sm">
              <Row term={msg("manager.status")} value={statusName(status())} />
              <Row term={msg("manager.category")} value={seat().category.label} />
              <Show when={object()?.order_ref}>
                {(ref) => <Row term={msg("manager.order")} value={ref()} />}
              </Show>
              <Show when={object()?.ticket_type}>
                {(t) => <Row term={msg("manager.ticketType")} value={t()} />}
              </Show>
              <Show when={object()?.expires_at}>
                {(at) => (
                  <Row term={msg("manager.expires")} value={dateTime.format(new Date(at()))} />
                )}
              </Show>
            </dl>
          </div>
        )}
      </Show>
    </section>
  );
}

function BookDialog(props: {
  open: boolean;
  labels: string[];
  onClose: () => void;
  onBook: (orderRef: string) => Promise<Outcome | null>;
}) {
  const [ref, setRef] = createSignal("");
  const [error, setError] = createSignal<string | null>(null);
  const [busy, setBusy] = createSignal(false);
  const check = () => {
    const v = ref().trim();
    const ok = v !== "" && length(v) <= 128 && !v.includes("\u0000");
    setError(ok ? null : msg("manager.orderRefRule"));
    return ok;
  };
  const submit = async (e: SubmitEvent) => {
    e.preventDefault();
    if (!check() || busy()) return;
    setBusy(true);
    try {
      const outcome = await props.onBook(ref().trim());
      if (outcome?.ok === true) setRef("");
    } finally {
      setBusy(false);
    }
  };
  return (
    <Show when={props.open}>
      <Dialog
        open
        heading
        title={msg("manager.bookTitle", { n: props.labels.length })}
        onClose={props.onClose}
      >
        <form class="space-y-4 px-5 pt-2 pb-5" noValidate onSubmit={(e) => void submit(e)}>
          <p class="text-base text-muted">
            {msg("manager.bookBody", { seats: seatsText(props.labels) })}
          </p>
          <Field
            label={msg("manager.orderRef")}
            hint={msg("manager.orderRefHint")}
            value={ref()}
            maxLength={128}
            onInput={(v) => {
              setRef(v);
              if (error() !== null) check();
            }}
            onBlur={() => check()}
            error={error()}
          />
          <div class="flex justify-end gap-2">
            <Button class="h-8" onClick={props.onClose}>
              {msg("action.cancel")}
            </Button>
            <Button type="submit" variant="primary" class="h-8" disabled={busy()}>
              {busy() ? msg("manager.busy") : msg("manager.bookConfirm")}
            </Button>
          </div>
        </form>
      </Dialog>
    </Show>
  );
}
