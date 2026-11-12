// The licence (09 §7 Settings → Licence, 12 §7.3): its state in words, the
// details for whoever manages the server, the upload, and the banners the
// console frame shows. What a person sees follows `me.licence.manage`; the
// state itself is everyone's.
import { A } from "@solidjs/router";
import { createSignal, createUniqueId, For, Match, Show, Switch } from "solid-js";
import { readLicence } from "../api/check.ts";
import { call } from "../api/client.ts";
import type { ConsoleLicence } from "../api/generated.ts";
import { asApiError, componentSignal } from "../api/query.ts";
import { msg } from "../i18n/messages.ts";
import { adoptLicence, loadSession, me, workspace } from "../session.ts";
import { Button } from "../ui/controls.tsx";
import { fieldInputClass, FormError } from "../ui/form.tsx";
import { notify } from "../ui/overlays.tsx";
import { describe, reference } from "./errors.ts";
import { bannerFor, graceEnd, licenceDate, reasonText } from "./licence.ts";

/** The largest licence file the server reads (12 §7.6). */
const MAX_BYTES = 16 * 1024;

const licence = (): ConsoleLicence | null => me()?.licence ?? null;
const canOpenSettings = () => workspace()?.permissions.includes("settings") === true;
const SETTINGS_PATH = "/settings?tab=licence";

/** The state in words, with what it means. */
function stateText(l: ConsoleLicence): string {
  switch (l.state) {
    case "valid":
      return l.licence === null
        ? msg("licence.valid")
        : msg("licence.validThrough", { date: licenceDate(l.licence.expires_at) });
    case "grace":
      return msg("licence.grace", { date: licenceDate(l.grace_until ?? "") });
    case "expired":
      return msg("licence.expired");
    case "missing":
      return msg("licence.missing");
    case "invalid":
      return msg("licence.invalid");
  }
}

/** Settings → Licence. */
export function LicenceTab() {
  return (
    <Show when={licence()}>
      {(l) => (
        <div class="max-w-2xl space-y-8">
          <section class="space-y-2" aria-labelledby="licence-state">
            <h2 id="licence-state" class="text-lg font-semibold text-fg">
              {msg(`licence.state.${l().state}`)}
            </h2>
            <p class="text-base text-fg">{stateText(l())}</p>
            <Show when={l().reason}>
              {(reason) => (
                <p class="text-base text-warning">
                  {msg("licence.why", { reason: reasonText(reason()) })}
                </p>
              )}
            </Show>
            <Show when={!l().manage}>
              <p class="text-base text-muted">{msg("licence.notManager")}</p>
            </Show>
          </section>
          <Show when={l().manage}>
            <Details licence={l()} />
            <Upload />
          </Show>
        </div>
      )}
    </Show>
  );
}

/** One term of the details and its value. */
const row = (term: string, value: string) => (
  <div class="grid grid-cols-[12rem_1fr] gap-3 border-b border-line py-2">
    <dt class="text-sm font-medium text-muted">{term}</dt>
    <dd class="text-base text-fg">{value}</dd>
  </div>
);

function Details(props: { licence: ConsoleLicence }) {
  return (
    <section class="space-y-2" aria-labelledby="licence-details">
      <h2 id="licence-details" class="text-lg font-semibold text-fg">
        {msg("licence.details")}
      </h2>
      <dl>
        <Show when={props.licence.licence}>
          {(d) => (
            <>
              {row(msg("licence.customer"), d().customer)}
              {row(msg("licence.plan"), d().plan)}
              {row(msg("licence.id"), d().id)}
              {row(msg("licence.issued"), licenceDate(d().issued_at))}
              {row(msg("licence.expires"), licenceDate(d().expires_at))}
              {row(
                msg("licence.graceUntil"),
                d().grace_days === 0
                  ? msg("licence.graceNone")
                  : licenceDate(graceEnd(d().expires_at, d().grace_days)),
              )}
              <div class="grid grid-cols-[12rem_1fr] gap-3 border-b border-line py-2">
                <dt class="text-sm font-medium text-muted">{msg("licence.domains")}</dt>
                <dd class="text-base text-fg">
                  <Show when={d().domains.length > 0} fallback={msg("licence.domainsNone")}>
                    <ul class="space-y-0.5">
                      <For each={d().domains}>
                        {(domain) => (
                          <li>
                            <span class="font-mono">{domain}</span>
                            <Show when={domain.startsWith("*.")}>
                              <span class="text-muted">
                                {" "}
                                ({msg("licence.domainUnder", { name: domain.slice(2) })})
                              </span>
                            </Show>
                          </li>
                        )}
                      </For>
                    </ul>
                  </Show>
                </dd>
              </div>
              {row(
                msg("licence.features"),
                d().features.length === 0 ? msg("licence.none") : d().features.join(", "),
              )}
              {row(
                msg("licence.workspaces"),
                d().max_workspaces === null ? msg("licence.unlimited") : String(d().max_workspaces),
              )}
              {row(msg("licence.key"), d().key_id)}
            </>
          )}
        </Show>
        <Show when={props.licence.source}>
          {(source) => row(msg("licence.source"), msg(`licence.source.${source()}`))}
        </Show>
      </dl>
      <Show when={props.licence.licence}>
        <p class="text-sm text-muted">{msg("licence.reported")}</p>
      </Show>
    </section>
  );
}

function Upload() {
  const signal = componentSignal();
  const fileId = createUniqueId();
  const pasteId = createUniqueId();
  const [text, setText] = createSignal("");
  const [fileName, setFileName] = createSignal<string | null>(null);
  const [error, setError] = createSignal<string | null>(null);
  const [failure, setFailure] = createSignal<{ text: string; detail?: string } | null>(null);
  const [busy, setBusy] = createSignal(false);
  let paste: HTMLTextAreaElement | undefined;

  const choose = async (file: File | undefined) => {
    setError(null);
    if (file === undefined) return;
    if (file.size > MAX_BYTES) {
      setFileName(null);
      return setError(msg("licence.tooLarge"));
    }
    try {
      setText(await file.text());
      setFileName(file.name);
    } catch {
      setError(msg("licence.unreadableFile"));
    }
  };
  const submit = async (e: SubmitEvent) => {
    e.preventDefault();
    if (busy()) return;
    setFailure(null);
    if (text().trim() === "") {
      setError(msg("licence.nothing"));
      return paste?.focus();
    }
    if (new TextEncoder().encode(text()).length > MAX_BYTES) {
      setError(msg("licence.tooLarge"));
      return paste?.focus();
    }
    setBusy(true);
    try {
      const answer = readLicence(
        await call("consoleLicence", { body: { licence: text() } }, signal),
      );
      adoptLicence(answer);
      setText("");
      setFileName(null);
      setError(null);
      notify(msg("licence.done", { state: msg(`licence.state.${answer.state}`) }));
    } catch (err) {
      const refusal = asApiError(err);
      if (signal.aborted || refusal.sessionLost) return;
      const problem = refusal.details["problem"];
      if (refusal.reason === "invalid_field" && refusal.details["field"] === "licence") {
        setError(reasonText(typeof problem === "string" ? problem : ""));
        paste?.focus();
      } else if (refusal.reason === "instance_owner") {
        setFailure({ text: msg("licence.ownerOnly") });
        void loadSession();
      } else {
        const detail = reference(refusal);
        setFailure(
          detail === undefined ? { text: describe(refusal) } : { text: describe(refusal), detail },
        );
      }
    } finally {
      setBusy(false);
    }
  };
  const described = () =>
    [`${pasteId}-hint`, error() ? `${pasteId}-error` : null].filter(Boolean).join(" ");
  return (
    <form class="space-y-4" noValidate onSubmit={(e) => void submit(e)}>
      <h2 class="text-lg font-semibold text-fg">{msg("licence.uploadTitle")}</h2>
      <p class="text-base text-muted">{msg("licence.uploadIntro")}</p>
      <div class="space-y-1">
        <label for={fileId} class="block text-sm font-medium text-fg">
          {msg("licence.file")}
        </label>
        <input
          id={fileId}
          type="file"
          accept=".json,application/json,text/plain"
          class="block text-base text-fg file:mr-3 file:h-8 file:rounded-md file:border file:border-line-strong file:bg-control file:px-2.5 file:text-base file:text-fg"
          aria-describedby={`${fileId}-hint`}
          onChange={(e) => void choose(e.currentTarget.files?.[0])}
        />
        <p id={`${fileId}-hint`} class="text-sm text-muted">
          {fileName() ?? msg("licence.fileHint")}
        </p>
      </div>
      <div class="space-y-1">
        <label for={pasteId} class="block text-sm font-medium text-fg">
          {msg("licence.paste")}
        </label>
        <textarea
          id={pasteId}
          ref={(el) => (paste = el)}
          rows={5}
          spellcheck={false}
          class={`${fieldInputClass} h-auto py-1.5 font-mono text-sm`}
          value={text()}
          aria-invalid={error() ? true : undefined}
          aria-describedby={described()}
          onInput={(e) => {
            setText(e.currentTarget.value);
            setFileName(null);
            setError(null);
          }}
        />
        <p id={`${pasteId}-hint`} class="text-sm text-muted">
          {msg("licence.fileHint")}
        </p>
        <Show when={error()}>
          <p id={`${pasteId}-error`} class="text-sm text-danger">
            {error()}
          </p>
        </Show>
      </div>
      <FormError message={failure()?.text ?? null} detail={failure()?.detail} />
      <Button type="submit" variant="primary" disabled={busy()}>
        {busy() ? msg("licence.submitting") : msg("licence.submit")}
      </Button>
    </form>
  );
}

/** Collapsed for this session, never for good (it comes back at the next sign-in). */
const HIDDEN_KEY = "cavea.licenceNotice";
const [hidden, setHidden] = createSignal(readHidden());
function readHidden(): boolean {
  try {
    return sessionStorage.getItem(HIDDEN_KEY) === "hidden";
  } catch {
    return false;
  }
}
function hide(on: boolean): void {
  setHidden(on);
  try {
    if (on) sessionStorage.setItem(HIDDEN_KEY, "hidden");
    else sessionStorage.removeItem(HIDDEN_KEY);
  } catch {
    // Hidden for this page only.
  }
}

const banner = () => {
  const l = licence();
  return l === null ? null : bannerFor(l, workspace()?.permissions ?? []);
};

/**
 * The frame's licence notice, above every console screen. In grace and
 * expired it stays; the development notice is one quiet line that can be
 * collapsed for the session into the navigation's `LicencePill`.
 */
export function LicenceBanner() {
  /** Where to act: the licence screen for whoever manages it, else whom to ask. */
  const next = () => {
    const l = licence();
    if (l?.manage === true && canOpenSettings()) {
      return (
        <A href={SETTINGS_PATH} class="font-medium text-accent underline">
          {msg("licence.open")}
        </A>
      );
    }
    if (l?.manage === true) return null;
    return (
      <span>
        {l?.state === "grace" || l?.state === "expired"
          ? msg("licence.askOwner")
          : msg("licence.askOwnerAdd")}
      </span>
    );
  };
  return (
    <Switch>
      <Match when={banner() === "grace" || banner() === "expired"}>
        <section
          aria-label={msg("licence.bannerLabel")}
          class="border-b border-warning bg-warning-soft px-8 py-2 text-base text-fg"
        >
          <span class="font-medium text-warning">
            {banner() === "grace"
              ? msg("licence.bannerGrace", { date: licenceDate(licence()?.grace_until ?? "") })
              : msg("licence.bannerExpired")}
          </span>{" "}
          {next()}
        </section>
      </Match>
      <Match when={banner() === "development" && !hidden()}>
        <section
          aria-label={msg("licence.bannerLabel")}
          class="flex flex-wrap items-center gap-x-3 gap-y-1 border-b border-line bg-app px-8 py-1.5 text-sm text-muted"
        >
          <span>{msg("licence.bannerDevelopment")}</span>
          {next()}
          <button
            type="button"
            class="ml-auto text-sm text-muted underline hover:text-fg"
            onClick={() => hide(true)}
          >
            {msg("licence.hide")}
          </button>
        </section>
      </Match>
    </Switch>
  );
}

/** The development notice, collapsed: a small control in the navigation that shows it again. */
export function LicencePill() {
  return (
    <Show when={banner() === "development" && hidden()}>
      <button
        type="button"
        class="w-full rounded-md border border-line px-2 py-1 text-left text-xs text-muted hover:text-fg"
        aria-label={msg("licence.showLabel")}
        onClick={() => hide(false)}
      >
        {msg("licence.show")}
      </button>
    </Show>
  );
}
