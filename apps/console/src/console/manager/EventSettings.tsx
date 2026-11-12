// An event's Settings tab (09 §4.3, CON-009): its name and every setting
// `PATCH /v1/events/{event}` takes, each with its unit and the server's
// limits, and archive. Only what differs from the server's defaults is
// sent: a setting left out goes back to its default (08 §6.2), which is
// the same value, and the event then follows later defaults as one created
// without it does. The chart-version upgrade, capacity overrides and
// channels are later tasks (CON-024, CON-022).
import { createSignal, Show } from "solid-js";
import { checkEvent } from "../../api/check.ts";
import { call } from "../../api/client.ts";
import type { Event, UpdateEvent } from "../../api/generated.ts";
import { asApiError, componentSignal } from "../../api/query.ts";
import { msg } from "../../i18n/messages.ts";
import { loadSession } from "../../session.ts";
import { Button } from "../../ui/controls.tsx";
import { Field, FormError, length } from "../../ui/form.tsx";
import { Confirm, notify } from "../../ui/overlays.tsx";
import { describe, reference, report } from "../errors.ts";
import { cannot } from "../licence.ts";
import { roleName } from "../Events.tsx";
import { Action, CheckField, NumberField } from "../kit.tsx";
import { EVENT_LIMITS, settingsRefusal, wholeNumber, type SettingsField } from "../refusals.ts";

const between = (text: string, limit: { min: number; max: number }) =>
  wholeNumber(text, limit.min, limit.max) === null ? msg("events.between", limit) : null;

type Errors = Partial<Record<SettingsField, string | null>>;

/** Why the settings cannot change now, if the role or the licence closes them. */
const settingsClosed = () =>
  cannot("updateEvent", () => msg("eventSettings.readOnly", { role: roleName() }));

export function EventSettings(props: { event: Event; onEvent: (event: Event) => void }) {
  const signal = componentSignal();
  const { holdTtl, maxObjects, tokensPerSub } = EVENT_LIMITS;
  const s = props.event.settings;
  const [name, setName] = createSignal(props.event.name ?? "");
  const [ttl, setTtl] = createSignal(String(s.hold_ttl_seconds));
  const [objects, setObjects] = createSignal(String(s.max_objects));
  const [session, setSession] = createSignal(s.require_session);
  const [tokens, setTokens] = createSignal(String(s.max_tokens_per_sub));
  const [errors, setErrors] = createSignal<Errors>({});
  const [failure, setFailure] = createSignal<{ text: string; detail?: string } | null>(null);
  const [busy, setBusy] = createSignal(false);
  const [archiving, setArchiving] = createSignal(false);

  const archived = () => props.event.archived_at !== null;
  /** `PATCH` and archive answer without counts: keep the ones shown. */
  const withCounts = (answer: Event): Event => {
    const counts = answer.counts ?? props.event.counts;
    return counts === undefined ? answer : { ...answer, counts };
  };
  const editable = () => settingsClosed() === null && !archived();

  const set = (field: SettingsField, problem: string | null) => {
    setErrors((e) => ({ ...e, [field]: problem }));
    return problem === null;
  };
  const checks: Record<SettingsField, () => boolean> = {
    name: () =>
      set(
        "name",
        name().trim() === "" || length(name().trim()) <= 128 ? null : msg("charts.textRule"),
      ),
    holdTtl: () => set("holdTtl", between(ttl(), holdTtl)),
    maxObjects: () => set("maxObjects", between(objects(), maxObjects)),
    tokens: () => set("tokens", session() ? between(tokens(), tokensPerSub) : null),
  };

  const body = (): UpdateEvent => {
    const settings: NonNullable<UpdateEvent["settings"]> = {};
    const t = Number(ttl());
    const o = Number(objects());
    if (t !== holdTtl.default) settings.hold_ttl_seconds = t;
    if (o !== maxObjects.default) settings.max_objects = o;
    if (session()) {
      settings.require_session = true;
      const k = Number(tokens());
      if (k !== tokensPerSub.default) settings.max_tokens_per_sub = k;
    }
    return { name: name().trim() === "" ? null : name().trim(), settings };
  };

  const submit = async (e: SubmitEvent) => {
    e.preventDefault();
    const ok = Object.values(checks)
      .map((check) => check())
      .every(Boolean);
    if (!ok || busy() || !editable()) return;
    setBusy(true);
    setFailure(null);
    try {
      const answer = checkEvent(
        await call("updateEvent", { path: { event: props.event.key }, body: body() }, signal),
      );
      props.onEvent(withCounts(answer));
      notify(msg("eventSettings.saved"));
    } catch (err) {
      const error = asApiError(err);
      if (signal.aborted || error.sessionLost) return;
      const refusal = settingsRefusal(error);
      if (refusal !== null) {
        set(refusal.field, refusal.message);
      } else {
        if (error.code === "forbidden") void loadSession();
        const text = error.reason === "archived" ? msg("eventSettings.archived") : describe(error);
        const detail = reference(error);
        setFailure(detail === undefined ? { text } : { text, detail });
      }
    } finally {
      setBusy(false);
    }
  };

  const archive = async () => {
    try {
      const answer = checkEvent(
        await call("archiveEvent", { path: { event: props.event.key } }, signal),
      );
      props.onEvent(withCounts(answer));
      notify(msg("events.archived", { name: answer.name ?? answer.key }));
    } catch (e) {
      report(e);
    }
  };

  const archiveBlocked = () =>
    cannot("archiveEvent", () => msg("eventSettings.archiveRole", { role: roleName() })) ??
    (archived() ? msg("eventSettings.alreadyArchived") : null);

  return (
    <div class="max-w-xl space-y-8">
      <form class="space-y-4" noValidate onSubmit={(e) => void submit(e)}>
        <h2 class="text-lg font-semibold text-fg">{msg("event.settings")}</h2>
        <p class="text-base text-muted">{msg("eventSettings.intro")}</p>
        <Show when={settingsClosed()}>{(why) => <p class="text-base text-muted">{why()}</p>}</Show>
        <Show when={archived()}>
          <p class="text-base text-warning">{msg("eventSettings.archivedNote")}</p>
        </Show>
        <fieldset disabled={!editable()} class="space-y-4">
          <Field
            label={msg("events.name")}
            hint={msg("events.nameHint")}
            value={name()}
            maxLength={128}
            onInput={(v) => {
              setName(v);
              if (errors().name) checks.name();
            }}
            onBlur={() => checks.name()}
            error={errors().name ?? null}
          />
          <NumberField
            label={msg("events.holdTtl")}
            hint={msg("events.holdTtlHint", holdTtl)}
            min={holdTtl.min}
            max={holdTtl.max}
            value={ttl()}
            onInput={(v) => {
              setTtl(v);
              if (errors().holdTtl) checks.holdTtl();
            }}
            onBlur={() => checks.holdTtl()}
            error={errors().holdTtl ?? null}
          />
          <NumberField
            label={msg("events.maxObjects")}
            hint={msg("events.maxObjectsHint", maxObjects)}
            min={maxObjects.min}
            max={maxObjects.max}
            value={objects()}
            onInput={(v) => {
              setObjects(v);
              if (errors().maxObjects) checks.maxObjects();
            }}
            onBlur={() => checks.maxObjects()}
            error={errors().maxObjects ?? null}
          />
          <CheckField
            label={msg("events.requireSession")}
            hint={msg("events.requireSessionHint")}
            checked={session()}
            onChange={setSession}
          />
          <Show when={session()}>
            <NumberField
              label={msg("events.tokens")}
              hint={msg("events.tokensHint", tokensPerSub)}
              min={tokensPerSub.min}
              max={tokensPerSub.max}
              value={tokens()}
              onInput={(v) => {
                setTokens(v);
                if (errors().tokens) checks.tokens();
              }}
              onBlur={() => checks.tokens()}
              error={errors().tokens ?? null}
            />
          </Show>
        </fieldset>
        <p class="text-sm text-muted">{msg("eventSettings.notYet")}</p>
        <FormError message={failure()?.text ?? null} detail={failure()?.detail} />
        <Show when={editable()}>
          <Button type="submit" variant="primary" class="h-8" disabled={busy()}>
            {busy() ? msg("eventSettings.saving") : msg("eventSettings.save")}
          </Button>
        </Show>
      </form>
      <section>
        <h2 class="text-lg font-semibold text-fg">{msg("eventSettings.archiveTitle")}</h2>
        <p class="mt-1 text-base text-muted">{msg("eventSettings.archiveBody")}</p>
        <Action
          class="mt-3 h-8"
          label={msg("events.archive")}
          icon="trash"
          variant="danger"
          blocked={archiveBlocked()}
          onClick={() => setArchiving(true)}
        />
      </section>
      <Confirm
        open={archiving()}
        title={msg("events.archiveTitle", { name: props.event.name ?? props.event.key })}
        description={msg("eventSettings.archiveBody")}
        confirm={msg("events.archiveConfirm")}
        variant="danger"
        onConfirm={() => void archive()}
        onClose={() => setArchiving(false)}
      />
    </div>
  );
}
