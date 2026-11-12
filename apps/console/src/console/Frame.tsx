// The console's frame (09 §1): left navigation and a content column of at
// most 1,280 px (09 §8), for a signed-in person only. Without a session it
// sends the person to sign-in and back here afterwards. The navigation
// shows the sections the workspace's role may open (CON-004); the
// workspace switcher sits under the logo when there is more than one.
import { DropdownMenu } from "@kobalte/core/dropdown-menu";
import { A, Navigate, useLocation, useNavigate } from "@solidjs/router";
import { For, Match, Show, Switch, type Accessor, type JSX, type ParentProps } from "solid-js";
import type { ConsoleMe } from "../api/generated.ts";
import type { Permission } from "../api/permissions.ts";
import { componentSignal } from "../api/query.ts";
import { msg, type MessageId } from "../i18n/messages.ts";
import { chooseWorkspace, me, session, signOut, workspace } from "../session.ts";
import { setThemeChoice, themeChoice, type ThemeChoice } from "../theme.ts";
import { Icon, type IconName } from "../ui/Icon.tsx";
import { menuContent, menuItem, menuSeparator } from "../ui/overlays.tsx";
import { report } from "./errors.ts";
import { LicenceBanner, LicencePill } from "./Licence.tsx";
import { ConsoleKeys } from "./Keys.tsx";
import { here, signInFor } from "./paths.ts";
import { Failed, Loading } from "./states.tsx";

export interface Section {
  path: string;
  label: MessageId;
  icon: IconName;
  /** What the role needs to open it: the permission of its list. */
  permission: Permission;
}

export const SECTIONS: Section[] = [
  { path: "/charts", label: "nav.charts", icon: "chart", permission: "charts.read" },
  { path: "/events", label: "nav.events", icon: "calendar", permission: "events.read" },
  { path: "/reports", label: "nav.reports", icon: "report", permission: "reports.read" },
  { path: "/developers", label: "nav.developers", icon: "code", permission: "developers" },
  { path: "/settings", label: "nav.settings", icon: "settings", permission: "settings" },
];
const THEMES: { value: ThemeChoice; icon: IconName }[] = [
  { value: "system", icon: "monitor" },
  { value: "light", icon: "sun" },
  { value: "dark", icon: "moon" },
];

/** Whether the selected workspace's role grants `permission`. */
export const may = (permission: Permission): boolean =>
  workspace()?.permissions.includes(permission) === true;

export function Frame(props: ParentProps) {
  return <SignedIn>{(m) => <Shell me={m()}>{props.children}</Shell>}</SignedIn>;
}

/** `children` for a signed-in person; otherwise the session's loading or
 * failed state, or sign-in and back here. For the frame and for the
 * full-window tools that need a session (the event manager). */
export function SignedIn(props: { children: (me: Accessor<ConsoleMe>) => JSX.Element }) {
  const location = useLocation();
  const back = () => {
    const s = session();
    // A sign-out starts afresh; a lost session comes back to this page.
    return s.kind === "signedOut" && s.cause === "left" ? "/sign-in" : signInFor(here(location));
  };
  return (
    <Switch>
      <Match when={session().kind === "loading"}>
        <Loading />
      </Match>
      <Match when={session().kind === "failed"}>
        <Failed />
      </Match>
      <Match when={session().kind === "signedOut"}>
        <Navigate href={back()} />
      </Match>
      <Match when={me()}>{(m) => props.children(m)}</Match>
    </Switch>
  );
}

function Shell(props: { me: ConsoleMe; children: JSX.Element }) {
  return (
    <div class="flex h-full">
      <nav
        aria-label={msg("nav.label")}
        class="flex w-56 shrink-0 flex-col border-r border-line bg-app p-2"
      >
        <div class="flex h-9 items-center gap-2 px-2 text-lg font-semibold text-fg">
          <span class="flex size-6 items-center justify-center rounded-md bg-accent-fill text-on-accent">
            <Icon name="section" class="size-3.5" />
          </span>
          {msg("app.name")}
        </div>
        <WorkspaceSwitcher me={props.me} />
        <ul class="mt-3 space-y-0.5">
          <For each={SECTIONS.filter((s) => may(s.permission))}>
            {(s) => (
              <li>
                <A
                  href={s.path}
                  class="flex h-8 items-center gap-2 rounded-md px-2 text-base text-muted hover:bg-hover hover:text-fg"
                  activeClass="bg-active text-fg font-medium"
                >
                  <Icon name={s.icon} />
                  {msg(s.label)}
                </A>
              </li>
            )}
          </For>
        </ul>
        <div class="mt-auto space-y-2 px-2 pb-1">
          <LicencePill />
          <div
            role="radiogroup"
            aria-label={msg("nav.theme")}
            class="flex rounded-md border border-line bg-surface p-0.5"
          >
            <For each={THEMES}>
              {(t) => (
                <button
                  type="button"
                  role="radio"
                  aria-checked={themeChoice() === t.value}
                  aria-label={msg(`theme.${t.value}`)}
                  title={msg(`theme.${t.value}`)}
                  class="flex h-6 flex-1 items-center justify-center rounded-sm text-muted hover:text-fg aria-checked:bg-active aria-checked:text-fg"
                  onClick={() => setThemeChoice(t.value)}
                >
                  <Icon name={t.icon} />
                </button>
              )}
            </For>
          </div>
          <UserMenu me={props.me} />
        </div>
      </nav>
      <main class="min-w-0 flex-1 overflow-y-auto bg-surface">
        <LicenceBanner />
        <div class="mx-auto max-w-[1280px] px-8 py-6">{props.children}</div>
      </main>
      <ConsoleKeys />
    </div>
  );
}

function WorkspaceSwitcher(props: { me: ConsoleMe }) {
  return (
    <Show
      when={props.me.workspaces.length > 1}
      fallback={
        <Show when={workspace()}>
          {(w) => <p class="truncate px-2 text-sm text-muted">{w().name}</p>}
        </Show>
      }
    >
      <DropdownMenu gutter={4} placement="bottom-start">
        <DropdownMenu.Trigger
          class="flex h-8 w-full items-center gap-2 rounded-md px-2 text-left text-base text-fg hover:bg-hover"
          aria-label={msg("workspace.switch", { name: workspace()?.name ?? "" })}
        >
          <span class="min-w-0 flex-1 truncate">{workspace()?.name}</span>
          <Icon name="chevronDown" />
        </DropdownMenu.Trigger>
        <DropdownMenu.Portal>
          <DropdownMenu.Content class={menuContent}>
            <DropdownMenu.Group>
              <DropdownMenu.GroupLabel class="px-2 py-1 text-sm text-muted">
                {msg("workspace.label")}
              </DropdownMenu.GroupLabel>
              <DropdownMenu.RadioGroup
                value={workspace()?.id ?? ""}
                onChange={(id) => chooseWorkspace(id)}
              >
                <For each={props.me.workspaces}>
                  {(w) => (
                    <DropdownMenu.RadioItem value={w.id} class={menuItem} closeOnSelect>
                      <span class="w-4">
                        <DropdownMenu.ItemIndicator>
                          <Icon name="check" />
                        </DropdownMenu.ItemIndicator>
                      </span>
                      <span class="min-w-0 flex-1 truncate">{w.name}</span>
                      <span class="pl-4 text-sm text-faint">{msg(`role.${w.role}`)}</span>
                    </DropdownMenu.RadioItem>
                  )}
                </For>
              </DropdownMenu.RadioGroup>
            </DropdownMenu.Group>
          </DropdownMenu.Content>
        </DropdownMenu.Portal>
      </DropdownMenu>
    </Show>
  );
}

function UserMenu(props: { me: ConsoleMe }) {
  const navigate = useNavigate();
  const signal = componentSignal();
  return (
    <DropdownMenu gutter={4} placement="top-start">
      <DropdownMenu.Trigger
        class="flex h-9 w-full items-center gap-2 rounded-md px-1 text-left text-base text-fg hover:bg-hover"
        aria-label={msg("user.menu", { email: props.me.user.email })}
      >
        <span
          class="flex size-6 shrink-0 items-center justify-center rounded-full bg-accent-soft text-sm font-semibold text-accent"
          aria-hidden="true"
        >
          {props.me.user.email.slice(0, 1).toUpperCase()}
        </span>
        <span class="min-w-0 flex-1">
          <span class="block truncate">{props.me.user.email}</span>
          <Show when={workspace()}>
            {(w) => <span class="block text-xs text-muted">{msg(`role.${w().role}`)}</span>}
          </Show>
        </span>
      </DropdownMenu.Trigger>
      <DropdownMenu.Portal>
        <DropdownMenu.Content class={menuContent}>
          <DropdownMenu.Item class={menuItem} onSelect={() => navigate("/account")}>
            {msg("user.account")}
          </DropdownMenu.Item>
          <DropdownMenu.Separator class={menuSeparator} />
          <DropdownMenu.Item class={menuItem} onSelect={() => signOut(signal).catch(report)}>
            {msg("user.signOut")}
          </DropdownMenu.Item>
        </DropdownMenu.Content>
      </DropdownMenu.Portal>
    </DropdownMenu>
  );
}

function NoWorkspace() {
  return (
    <>
      <h1 class="text-xl font-semibold text-fg">{msg("workspace.noneTitle")}</h1>
      <p class="mt-2 max-w-xl text-base text-muted">{msg("workspace.noneBody")}</p>
    </>
  );
}

/** A section's page, for the selected workspace: without one, why not;
 * without the permission, why not (a link from elsewhere, or a role lowered
 * since). A workspace switch remounts the page, so its requests are
 * cancelled and nothing of the previous workspace stays on screen. The
 * account page is outside it: a person in no workspace still has one. */
export function Gate(props: ParentProps<{ section: Section }>) {
  return (
    <Show when={workspace()?.id} keyed fallback={<NoWorkspace />}>
      {(_id) => <Allowed section={props.section}>{props.children}</Allowed>}
    </Show>
  );
}

function Allowed(props: ParentProps<{ section: Section }>) {
  return (
    <Show
      when={may(props.section.permission)}
      fallback={
        <>
          <h1 class="text-xl font-semibold text-fg">
            {msg("gate.title", { section: msg(props.section.label) })}
          </h1>
          <p class="mt-2 text-base text-muted">
            {msg("gate.body", { role: msg(`role.${workspace()?.role ?? "viewer"}`) })}
          </p>
        </>
      }
    >
      {props.children}
    </Show>
  );
}

export function NotBuilt(props: { section: MessageId }) {
  return (
    <>
      <h1 class="text-xl font-semibold text-fg">
        {msg("notBuilt.title", { section: msg(props.section) })}
      </h1>
      <p class="mt-2 text-base text-muted">{msg("notBuilt.body")}</p>
    </>
  );
}

export function NotFound() {
  return (
    <>
      <h1 class="text-xl font-semibold text-fg">{msg("notFound.title")}</h1>
      <A href="/charts" class="mt-2 inline-block text-base text-accent underline">
        {msg("notFound.back")}
      </A>
    </>
  );
}
