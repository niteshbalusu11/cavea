// The form for a new event (09 §4.1, CON-007): key, name, chart, version
// (the chart's published version unless another is chosen) and the
// settings the API takes, each with its unit and the server's limits.
// Opened from the events list.
import { createMemo, createSignal, Show } from "solid-js";
import { checkEvent, checkVersionList } from "../api/check.ts";
import { call } from "../api/client.ts";
import type { Chart, CreateEvent, Event, Version } from "../api/generated.ts";
import { loadAll } from "../api/pages.ts";
import { asApiError, componentSignal, createQuery } from "../api/query.ts";
import { formatNumber, msg } from "../i18n/messages.ts";
import { loadSession, workspace } from "../session.ts";
import { Button } from "../ui/controls.tsx";
import { Field, FormError, length } from "../ui/form.tsx";
import { Dialog } from "../ui/overlays.tsx";
import { keyFrom } from "./Charts.tsx";
import { describe, reference } from "./errors.ts";
import { CheckField, formatDate, NumberField, SelectField } from "./kit.tsx";
import { EVENT_LIMITS, eventRefusal, wholeNumber, type EventField } from "./refusals.ts";

/** At most this many pages of a chart's versions are offered. */
const VERSION_PAGES = 20;

/** A chart's versions, newest first, whole up to `VERSION_PAGES` pages. */
async function loadVersions(chart: string, signal: AbortSignal) {
  const all = await loadAll(
    async (after) =>
      checkVersionList(
        await call(
          "listVersions",
          { path: { chart }, query: after === undefined ? { limit: 100 } : { limit: 100, after } },
          signal,
        ),
      ),
    VERSION_PAGES,
  );
  // oxlint-disable-next-line unicorn/no-array-reverse -- reverses a copy of ours; `toReversed` is ES2023, past the lib (ES2022).
  return { versions: [...all.data].reverse(), complete: all.complete };
}

function versionLabel(v: Version, published: number | null): string {
  const text = msg("events.versionOption", {
    number: v.number,
    seats: formatNumber(v.stats.seats),
    date: formatDate(v.created_at),
  });
  return v.number === published ? msg("events.versionPublished", { text }) : text;
}

/** The server's text rule: 1 to 128 characters (none, when optional). */
const textProblem = (v: string, optional: boolean) =>
  (optional && v.trim() === "") || (v.trim() !== "" && length(v.trim()) <= 128)
    ? null
    : msg("charts.textRule");

type Errors = Partial<Record<EventField, string | null>>;

export function EventForm(props: {
  /** The charts to choose from: every one events can be made on. */
  charts: Chart[];
  onClose: () => void;
  onCreated: (event: Event) => void;
}) {
  const signal = componentSignal();
  const { holdTtl, maxObjects, tokensPerSub } = EVENT_LIMITS;
  const usable = () =>
    props.charts.filter((c) => c.archived_at === null && c.published_version !== null);
  const [chartKey, setChartKey] = createSignal("");
  const [version, setVersion] = createSignal("");
  const [name, setName] = createSignal("");
  const [key, setKey] = createSignal("");
  const [keyEdited, setKeyEdited] = createSignal(false);
  const [ttl, setTtl] = createSignal(String(holdTtl.default));
  const [objects, setObjects] = createSignal(String(maxObjects.default));
  const [tokens, setTokens] = createSignal(String(tokensPerSub.default));
  const [session, setSession] = createSignal(false);
  const [errors, setErrors] = createSignal<Errors>({});
  const [failure, setFailure] = createSignal<{ text: string; detail?: string } | null>(null);
  const [busy, setBusy] = createSignal(false);

  const chart = createMemo(() => props.charts.find((c) => c.key === chartKey()) ?? null);
  const published = () => chart()?.published_version?.number ?? null;
  const versions = createQuery(
    () => {
      const c = chart();
      const ws = workspace()?.id;
      return c === null || ws === undefined ? null : `${ws}/${c.key}`;
    },
    (k, s) => loadVersions(k.slice(k.indexOf("/") + 1), s),
  );
  // The published version unless one was chosen; reset with the chart.
  const chosenVersion = () => (version() === "" ? String(published() ?? "") : version());

  const set = (field: EventField, problem: string | null) => {
    setErrors((e) => ({ ...e, [field]: problem }));
    return problem === null;
  };
  const checks: Record<EventField, () => boolean> = {
    key: () => set("key", textProblem(key(), false)),
    name: () => set("name", textProblem(name(), true)),
    chart: () => set("chart", chart() === null ? msg("events.chartMissing") : null),
    version: () => set("version", chosenVersion() === "" ? msg("events.versionMissing") : null),
    holdTtl: () =>
      set(
        "holdTtl",
        wholeNumber(ttl(), holdTtl.min, holdTtl.max) === null
          ? msg("events.between", holdTtl)
          : null,
      ),
    maxObjects: () =>
      set(
        "maxObjects",
        wholeNumber(objects(), maxObjects.min, maxObjects.max) === null
          ? msg("events.between", maxObjects)
          : null,
      ),
    tokens: () =>
      set(
        "tokens",
        session() && wholeNumber(tokens(), tokensPerSub.min, tokensPerSub.max) === null
          ? msg("events.between", tokensPerSub)
          : null,
      ),
  };

  /** The settings that differ from the server's defaults: an event keeps
   * only what was set, and takes later defaults for the rest. */
  const settings = (): NonNullable<CreateEvent["settings"]> => {
    const out: NonNullable<CreateEvent["settings"]> = {};
    const t = Number(ttl());
    const o = Number(objects());
    const k = Number(tokens());
    if (t !== holdTtl.default) out.hold_ttl_seconds = t;
    if (o !== maxObjects.default) out.max_objects = o;
    if (session()) {
      out.require_session = true;
      if (k !== tokensPerSub.default) out.max_tokens_per_sub = k;
    }
    return out;
  };

  const submit = async (e: SubmitEvent) => {
    e.preventDefault();
    const ok = Object.values(checks)
      .map((check) => check())
      .every(Boolean);
    const c = chart();
    if (!ok || busy() || c === null) return;
    setBusy(true);
    setFailure(null);
    try {
      const body: CreateEvent = {
        key: key().trim(),
        chart: c.key,
        version: Number(chosenVersion()),
        settings: settings(),
      };
      if (name().trim() !== "") body.name = name().trim();
      props.onCreated(checkEvent(await call("createEvent", { body }, signal)));
    } catch (err) {
      const error = asApiError(err);
      if (signal.aborted || error.sessionLost) return;
      const refusal = eventRefusal(error);
      if (refusal !== null) {
        set(refusal.field, refusal.message);
      } else {
        if (error.code === "forbidden") void loadSession();
        const detail = reference(error);
        setFailure(
          detail === undefined ? { text: describe(error) } : { text: describe(error), detail },
        );
      }
    } finally {
      setBusy(false);
    }
  };

  const versionOptions = () => {
    const s = versions.state();
    if (s.status !== "ready") return [];
    return s.data.versions.map((v) => ({
      value: String(v.number),
      label: versionLabel(v, published()),
    }));
  };

  return (
    <Dialog open title={msg("events.newTitle")} heading class="max-w-lg" onClose={props.onClose}>
      <form
        class="space-y-4 overflow-y-auto px-5 pt-2 pb-5"
        noValidate
        onSubmit={(e) => void submit(e)}
      >
        <Field
          label={msg("events.name")}
          hint={msg("events.nameHint")}
          value={name()}
          maxLength={128}
          onInput={(v) => {
            setName(v);
            if (!keyEdited()) setKey(keyFrom(v));
            if (errors().name) checks.name();
          }}
          onBlur={() => checks.name()}
          error={errors().name ?? null}
        />
        <Field
          label={msg("events.key")}
          hint={msg("events.keyHint")}
          mono
          value={key()}
          maxLength={128}
          onInput={(v) => {
            setKey(v);
            setKeyEdited(true);
            if (errors().key) checks.key();
          }}
          onBlur={() => checks.key()}
          error={errors().key ?? null}
        />
        <SelectField
          label={msg("events.chart")}
          value={chartKey()}
          placeholder={msg("events.chartPlaceholder")}
          options={usable().map((c) => ({ value: c.key, label: `${c.name} (${c.key})` }))}
          hint={usable().length === 0 ? msg("events.noCharts") : msg("events.chartHint")}
          onChange={(v) => {
            setChartKey(v);
            setVersion("");
            checks.chart();
          }}
          error={errors().chart ?? null}
        />
        <Show when={chart() !== null}>
          <Show
            when={versions.state().status !== "error"}
            fallback={
              <FormError
                message={msg("events.versionsFailed")}
                detail={(() => {
                  const s = versions.state();
                  return s.status === "error" ? describe(s.error) : undefined;
                })()}
              />
            }
          >
            <SelectField
              label={msg("events.version")}
              value={versions.state().status === "ready" ? chosenVersion() : ""}
              placeholder={msg("events.versionsLoading")}
              options={versionOptions()}
              disabled={versions.state().status !== "ready"}
              hint={(() => {
                const s = versions.state();
                return s.status === "ready" && !s.data.complete
                  ? msg("events.versionsPartial", { n: s.data.versions.length })
                  : msg("events.versionHint");
              })()}
              onChange={(v) => {
                setVersion(v);
                checks.version();
              }}
              error={errors().version ?? null}
            />
          </Show>
        </Show>
        <fieldset class="space-y-4 rounded-md border border-line p-3">
          <legend class="px-1 text-sm font-medium text-fg">{msg("events.settings")}</legend>
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
          <p class="text-sm text-muted">{msg("events.notYet")}</p>
        </fieldset>
        <FormError message={failure()?.text ?? null} detail={failure()?.detail} />
        <div class="flex justify-end gap-2">
          <Button class="h-8" onClick={props.onClose}>
            {msg("action.cancel")}
          </Button>
          <Button type="submit" variant="primary" class="h-8" disabled={busy()}>
            {busy() ? msg("events.creating") : msg("events.create")}
          </Button>
        </div>
      </form>
    </Dialog>
  );
}
