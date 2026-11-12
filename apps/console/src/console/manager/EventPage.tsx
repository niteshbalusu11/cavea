// An event's page (09 §4.2, §4.3): a full-window tool outside the console
// frame, with the event's name, chart, counts and tabs in a bar, and the
// manager or the settings under it. It needs a session and the events
// section's permission, as the frame's pages do; a workspace switch
// remounts it, so nothing of the previous workspace's event keeps running.
import { A, useMatch, useParams } from "@solidjs/router";
import { Match, Show, Switch, type JSX } from "solid-js";
import { checkEvent } from "../../api/check.ts";
import { call } from "../../api/client.ts";
import type { Event } from "../../api/generated.ts";
import { createQuery, type Query } from "../../api/query.ts";
import { formatNumber, msg } from "../../i18n/messages.ts";
import { workspace } from "../../session.ts";
import { Gate, may, SECTIONS, SignedIn } from "../Frame.tsx";
import { ListError, ListLoading } from "../kit.tsx";
import { EventSettings } from "./EventSettings.tsx";
import { Manager } from "./Manager.tsx";
import { LicenceBanner } from "../Licence.tsx";
import { ConsoleKeys } from "../Keys.tsx";

const EVENTS = SECTIONS.find((s) => s.path === "/events");

/** Content at the frame's padding, for the states before the tool. */
const Pad = (props: { children: JSX.Element }) => <div class="p-6">{props.children}</div>;

export function EventPage() {
  return (
    <SignedIn>
      {() =>
        EVENTS === undefined ? null : (
          <div class="flex h-full flex-col bg-app text-fg">
            <LicenceBanner />
            <Show
              when={workspace()?.id !== undefined && may(EVENTS.permission) && workspace()?.id}
              keyed
              fallback={
                <Pad>
                  <Gate section={EVENTS}>{null}</Gate>
                </Pad>
              }
            >
              {(_ws) => <EventScreen />}
            </Show>
            <ConsoleKeys />
          </div>
        )
      }
    </SignedIn>
  );
}

function EventScreen() {
  const params = useParams<{ event: string }>();
  const query = createQuery(
    () => params.event,
    async (event, s) => checkEvent(await call("getEvent", { path: { event } }, s)),
  );
  return (
    <Switch>
      <Match when={query.state().status === "loading"}>
        <Pad>
          <ListLoading label={msg("event.loading")} rows={2} />
        </Pad>
      </Match>
      <Match
        when={(() => {
          const s = query.state();
          return s.status === "error" ? s.error : null;
        })()}
      >
        {(e) => (
          <Pad>
            <A href="/events" class="text-sm text-accent underline">
              {msg("event.back")}
            </A>
            <Show
              when={e().code !== "not_found"}
              fallback={<h1 class="mt-3 text-xl font-semibold">{msg("event.notFound")}</h1>}
            >
              <ListError title={msg("event.failed")} error={e()} onRetry={query.refetch} />
            </Show>
          </Pad>
        )}
      </Match>
      <Match
        when={(() => {
          const s = query.state();
          return s.status === "ready" ? s.data : null;
        })()}
      >
        {(event) => <Loaded event={event()} query={query} />}
      </Match>
    </Switch>
  );
}

const tab = (active: boolean) =>
  `flex h-12 items-center border-b-2 px-3 text-base ${
    active ? "border-accent font-medium text-fg" : "border-transparent text-muted hover:text-fg"
  }`;

function Loaded(props: { event: Event; query: Query<Event> }) {
  const onSettings = useMatch(() => "/events/:event/settings");
  const settings = () => onSettings() !== undefined;
  const base = () => `/events/${encodeURIComponent(props.event.key)}`;
  return (
    <>
      <header class="flex shrink-0 flex-wrap items-center gap-x-6 gap-y-1 border-b border-line bg-surface px-4">
        <A href="/events" class="text-sm text-accent underline">
          {msg("event.back")}
        </A>
        <div class="min-w-0 py-1.5">
          <h1 class="truncate text-lg leading-tight font-semibold">
            {props.event.name ?? props.event.key}
            <Show when={props.event.archived_at !== null}>
              <span class="ml-2 rounded-sm bg-warning-soft px-1.5 align-middle text-xs font-normal text-warning">
                {msg("events.archivedBadge")}
              </span>
            </Show>
          </h1>
          <p class="truncate text-sm text-muted">
            <span class="font-mono">{props.event.key}</span>
            {" · "}
            <A href={`/charts/${encodeURIComponent(props.event.chart)}`} class="underline">
              {msg("event.chartVersion", {
                chart: props.event.chart,
                number: props.event.version.number,
              })}
            </A>
          </p>
        </div>
        <Show when={props.event.counts}>
          {(c) => (
            <p class="text-sm text-muted" aria-live="polite">
              {msg("event.counts", {
                booked: formatNumber(c().booked),
                held: formatNumber(c().held),
                blocked: formatNumber(c().blocked),
                free: formatNumber(c().free),
              })}
              {Object.entries(c().custom)
                .filter(([, n]) => n > 0)
                .map(([code, n]) => ` · ${msg("event.customCount", { n: formatNumber(n), code })}`)
                .join("")}
            </p>
          )}
        </Show>
        <nav aria-label={msg("event.tabs")} class="ml-auto flex self-end">
          <A href={base()} class={tab(!settings())} end>
            {msg("event.manager")}
          </A>
          <A href={`${base()}/settings`} class={tab(settings())}>
            {msg("event.settings")}
          </A>
        </nav>
      </header>
      <Show
        when={settings()}
        fallback={<Manager event={props.event} onEvent={(e) => props.query.update(() => e)} />}
      >
        <div class="min-h-0 flex-1 overflow-y-auto p-6">
          <EventSettings event={props.event} onEvent={(e) => props.query.update(() => e)} />
        </div>
      </Show>
    </>
  );
}
