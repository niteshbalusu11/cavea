// Developers (09 §6): the API keys screen (CON-010) and the quick start
// (CON-011). A created key is shown once, in a dialog that says so, and is
// gone from memory when it closes: the server keeps only its hash, so the
// list shows a key's prefix. The quick start never shows a secret key; a
// publishable key appears in it only while its creation dialog is open or
// when pasted (it is not stored).
import { useSearchParams } from "@solidjs/router";
import { createMemo, createSignal, For, Show } from "solid-js";
import { checkKey, checkKeyCreated, checkKeyList } from "../api/check.ts";
import { call } from "../api/client.ts";
import type { CreateKey, Key, KeyCreated } from "../api/generated.ts";
import { createPages } from "../api/pages.ts";
import { allows } from "../api/permissions.ts";
import { asApiError, componentSignal } from "../api/query.ts";
import { msg } from "../i18n/messages.ts";
import { loadSession, workspace } from "../session.ts";
import { Button } from "../ui/controls.tsx";
import { Field, fieldInputClass, FormError, length } from "../ui/form.tsx";
import { Confirm, Dialog, notify } from "../ui/overlays.tsx";
import { describe, reference, report } from "./errors.ts";
import { roleName } from "./Events.tsx";
import {
  Action,
  CopyField,
  Empty,
  formatDateTime,
  LoadMore,
  Paged,
  Tabs,
  table,
  td,
  th,
  TextAreaField,
} from "./kit.tsx";
import {
  badOrigins,
  keyRefusal,
  originsProblem,
  originsRefusal,
  parseOrigins,
  type KeyField,
} from "./refusals.ts";
import { pastedKey, snippets, type Snippet } from "./snippets.ts";

type Tab = "keys" | "quickstart";

export function Developers() {
  const [params, setParams] = useSearchParams<{ tab?: string }>();
  const tab = (): Tab => (params.tab === "quickstart" ? "quickstart" : "keys");
  return (
    <>
      <h1 class="text-xl font-semibold text-fg">{msg("developers.title")}</h1>
      <Tabs
        label={msg("developers.title")}
        value={tab()}
        onChange={(t) => setParams({ tab: t === "keys" ? undefined : t }, { replace: true })}
        tabs={[
          { value: "keys", label: msg("keys.title"), content: () => <Keys /> },
          { value: "quickstart", label: msg("quick.title"), content: () => <QuickStart /> },
        ]}
      />
    </>
  );
}

const cannotEditOrigins = () =>
  allows(workspace(), "updateKeyOrigins") ? null : msg("keys.roleCannot", { role: roleName() });
const cannotManage = () =>
  allows(workspace(), "createKey") ? null : msg("keys.roleCannot", { role: roleName() });

function Keys() {
  const signal = componentSignal();
  const keys = createPages(
    () => workspace()?.id ?? null,
    async (_ws, after, s) =>
      checkKeyList(
        await call(
          "listKeys",
          { query: after === undefined ? { limit: 50 } : { limit: 50, after } },
          s,
        ),
      ),
  );
  const [revoked, setRevoked] = createSignal(false);
  const [creating, setCreating] = createSignal(false);
  const [revoking, setRevoking] = createSignal<Key | null>(null);
  const [editing, setEditing] = createSignal<Key | null>(null);

  const revoke = async (key: Key) => {
    try {
      const answer = checkKey(await call("revokeKey", { path: { id: key.id } }, signal));
      keys.update((rows) => rows.map((r) => (r.id === answer.id ? answer : r)));
      notify(msg("keys.revoked", { name: key.name }));
    } catch (e) {
      report(e);
    }
  };

  return (
    <>
      <div class="flex flex-wrap items-center gap-3">
        <p class="mr-auto max-w-2xl text-base text-muted">{msg("keys.intro")}</p>
        <Action
          label={msg("keys.new")}
          icon="plus"
          variant="primary"
          blocked={cannotManage()}
          onClick={() => setCreating(true)}
        />
      </div>
      <Paged pages={keys} loading={msg("keys.loading")} failed={msg("keys.failed")}>
        {(rows) => (
          <Show
            when={rows().length > 0}
            fallback={
              <Empty icon="code" title={msg("keys.emptyTitle")} body={msg("keys.emptyBody")} />
            }
          >
            <label class="mt-4 flex items-center gap-2 text-base text-fg">
              <input
                type="checkbox"
                class="size-4 accent-accent-fill"
                checked={revoked()}
                onChange={(e) => setRevoked(e.currentTarget.checked)}
              />
              {msg("keys.showRevoked")}
            </label>
            <div class="mt-3 overflow-x-auto">
              <table class={table}>
                <caption class="sr-only">{msg("keys.title")}</caption>
                <thead>
                  <tr>
                    <th scope="col" class={th}>
                      {msg("keys.colName")}
                    </th>
                    <th scope="col" class={th}>
                      {msg("keys.colType")}
                    </th>
                    <th scope="col" class={th}>
                      {msg("keys.colPrefix")}
                    </th>
                    <th scope="col" class={th}>
                      {msg("keys.colOrigins")}
                    </th>
                    <th scope="col" class={th}>
                      {msg("keys.colCreated")}
                    </th>
                    <th scope="col" class={th}>
                      {msg("keys.colLastUsed")}
                    </th>
                    <th scope="col" class={th}>
                      <span class="sr-only">{msg("events.colActions")}</span>
                    </th>
                  </tr>
                </thead>
                <tbody>
                  <For each={rows().filter((k) => revoked() || k.revoked_at === null)}>
                    {(key) => (
                      <KeyRow
                        key={key}
                        onRevoke={() => setRevoking(key)}
                        onEdit={() => setEditing(key)}
                      />
                    )}
                  </For>
                </tbody>
              </table>
            </div>
            <Show when={!revoked() && rows().every((k) => k.revoked_at !== null)}>
              <p class="mt-3 text-base text-muted">{msg("keys.allRevoked")}</p>
            </Show>
            <LoadMore pages={keys} />
          </Show>
        )}
      </Paged>
      <Show when={creating()}>
        <KeyDialog onClose={() => setCreating(false)} onCreated={() => keys.refetch()} />
      </Show>
      <Show when={editing()} keyed>
        {(key) => (
          <OriginsDialog
            apiKey={key}
            onClose={() => setEditing(null)}
            onSaved={(answer) => {
              keys.update((rows) => rows.map((r) => (r.id === answer.id ? answer : r)));
              notify(msg("keys.originsSaved", { name: answer.name }));
              setEditing(null);
            }}
            onGone={() => keys.refetch()}
          />
        )}
      </Show>
      <Confirm
        open={revoking() !== null}
        title={msg("keys.revokeTitle", { name: revoking()?.name ?? "" })}
        description={msg("keys.revokeBody", { prefix: `${revoking()?.prefix ?? ""}…` })}
        confirm={msg("keys.revokeConfirm")}
        variant="danger"
        onConfirm={() => {
          const key = revoking();
          if (key !== null) void revoke(key);
        }}
        onClose={() => setRevoking(null)}
      />
    </>
  );
}

function KeyRow(props: { key: Key; onRevoke: () => void; onEdit: () => void }) {
  const live = () => props.key.revoked_at === null;
  return (
    <tr classList={{ "text-muted": !live() }}>
      <td class={td}>
        <span class="font-medium">{props.key.name}</span>
        <Show when={!live()}>
          <span class="ml-2 rounded-sm bg-warning-soft px-1.5 text-xs text-warning">
            {msg("keys.revokedBadge", { when: formatDateTime(props.key.revoked_at ?? "") })}
          </span>
        </Show>
      </td>
      <td class={td}>{msg(props.key.kind === "secret" ? "keys.secret" : "keys.publishable")}</td>
      <td class={`${td} font-mono text-sm`}>{props.key.prefix}…</td>
      <td class={`${td} text-sm`}>
        <Show when={props.key.kind === "publishable"} fallback="—">
          <Show when={props.key.allowed_origins} fallback={msg("keys.anyOrigin")}>
            {(list) => (
              <ul>
                <For each={list()}>{(o) => <li class="font-mono">{o}</li>}</For>
              </ul>
            )}
          </Show>
        </Show>
      </td>
      <td class={`${td} text-sm whitespace-nowrap`}>{formatDateTime(props.key.created_at)}</td>
      <td class={`${td} text-sm whitespace-nowrap`}>
        {props.key.last_used_at === null
          ? msg("keys.never")
          : formatDateTime(props.key.last_used_at)}
      </td>
      <td class={`${td} text-right whitespace-nowrap`}>
        <Show when={live() && props.key.kind === "publishable"}>
          <span data-focus-id={`origins-${props.key.id}`}>
            <Action
              label={msg("keys.editOrigins")}
              class="mr-2 h-7"
              blocked={cannotEditOrigins()}
              onClick={props.onEdit}
            />
          </span>
        </Show>
        <Show when={live()}>
          <span data-focus-id={`revoke-${props.key.id}`}>
            <Action
              label={msg("keys.revoke")}
              class="h-7"
              variant="danger"
              blocked={cannotManage()}
              onClick={props.onRevoke}
            />
          </span>
        </Show>
      </td>
    </tr>
  );
}

type Errors = Partial<Record<KeyField, string | null>>;

/** Creates a key, then shows it once. */
function KeyDialog(props: { onClose: () => void; onCreated: () => void }) {
  const signal = componentSignal();
  const [name, setName] = createSignal("");
  const [kind, setKind] = createSignal<CreateKey["kind"]>("secret");
  const [origins, setOrigins] = createSignal("");
  const [errors, setErrors] = createSignal<Errors>({});
  const [failure, setFailure] = createSignal<{ text: string; detail?: string } | null>(null);
  const [busy, setBusy] = createSignal(false);
  // The key itself: held only while the dialog is open.
  const [created, setCreated] = createSignal<KeyCreated | null>(null);

  const set = (field: KeyField, problem: string | null) => {
    setErrors((e) => ({ ...e, [field]: problem }));
    return problem === null;
  };
  const checkName = () =>
    set(
      "name",
      name().trim() === "" || length(name().trim()) > 128 ? msg("charts.textRule") : null,
    );
  const checkOrigins = () => {
    if (kind() !== "publishable") return set("origins", null);
    const list = parseOrigins(origins()) ?? [];
    const bad = badOrigins(list);
    if (list.length > 50) return set("origins", msg("keys.originsTooMany"));
    return set("origins", bad.length > 0 ? msg("keys.originsBad", { list: bad.join(", ") }) : null);
  };

  const submit = async (e: SubmitEvent) => {
    e.preventDefault();
    if (![checkName(), checkOrigins()].every(Boolean) || busy()) return;
    setBusy(true);
    setFailure(null);
    try {
      const body: CreateKey = { kind: kind(), name: name().trim() };
      const list = kind() === "publishable" ? parseOrigins(origins()) : null;
      if (list !== null) body.allowed_origins = list;
      setCreated(checkKeyCreated(await call("createKey", { body }, signal)));
      props.onCreated();
    } catch (err) {
      const error = asApiError(err);
      if (signal.aborted || error.sessionLost) return;
      const refusal = keyRefusal(error);
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

  const close = () => {
    setCreated(null);
    props.onClose();
  };

  return (
    <Show
      when={created()}
      fallback={
        <Dialog open title={msg("keys.newTitle")} heading onClose={close}>
          <form class="space-y-4 px-5 pt-2 pb-5" noValidate onSubmit={(e) => void submit(e)}>
            <Field
              label={msg("keys.name")}
              hint={msg("keys.nameHint")}
              value={name()}
              maxLength={128}
              onInput={(v) => {
                setName(v);
                if (errors().name) checkName();
              }}
              onBlur={checkName}
              error={errors().name ?? null}
            />
            <fieldset class="space-y-2">
              <legend class="text-sm font-medium text-fg">{msg("keys.kind")}</legend>
              <For each={["secret", "publishable"] as const}>
                {(k) => (
                  <label class="flex items-start gap-2 text-base text-fg">
                    <input
                      type="radio"
                      name="kind"
                      class="mt-1 size-4 accent-accent-fill"
                      checked={kind() === k}
                      onChange={() => {
                        setKind(k);
                        checkOrigins();
                      }}
                    />
                    <span>
                      <span class="block">
                        {msg(k === "secret" ? "keys.secret" : "keys.publishable")}
                      </span>
                      <span class="block text-sm text-muted">
                        {msg(k === "secret" ? "keys.secretHint" : "keys.publishableHint")}
                      </span>
                    </span>
                  </label>
                )}
              </For>
            </fieldset>
            <Show when={kind() === "publishable"}>
              <TextAreaField
                label={msg("keys.origins")}
                hint={msg("keys.originsHint")}
                placeholder="https://tickets.example.com"
                value={origins()}
                onInput={(v) => {
                  setOrigins(v);
                  if (errors().origins) checkOrigins();
                }}
                onBlur={checkOrigins}
                error={errors().origins ?? null}
              />
            </Show>
            <FormError message={failure()?.text ?? null} detail={failure()?.detail} />
            <div class="flex justify-end gap-2">
              <Button class="h-8" onClick={close}>
                {msg("action.cancel")}
              </Button>
              <Button type="submit" variant="primary" class="h-8" disabled={busy()}>
                {busy() ? msg("keys.creating") : msg("keys.create")}
              </Button>
            </div>
          </form>
        </Dialog>
      }
    >
      {(key) => <KeyShown created={key()} onClose={close} />}
    </Show>
  );
}

/** The created key, once: copy it now. A publishable key's quick start is
 * filled with it here, where it may be shown. */
function KeyShown(props: { created: KeyCreated; onClose: () => void }) {
  const ws = workspace();
  return (
    <Confirm
      open
      title={msg("keys.shownTitle", { name: props.created.name })}
      description={msg("keys.shownBody")}
      onClose={props.onClose}
    >
      <div class="mt-4 space-y-4">
        <CopyField
          label={msg(props.created.kind === "secret" ? "keys.secret" : "keys.publishable")}
          value={props.created.key}
        />
        <Show when={props.created.kind === "publishable" && ws !== null}>
          <SnippetBlock
            snippet={
              snippets({
                origin: window.location.origin,
                workspace: { key: ws?.key ?? "", name: ws?.name ?? "" },
                publishable: props.created.key,
              })[1] as Snippet
            }
          />
        </Show>
      </div>
    </Confirm>
  );
}

/** Changes a publishable key's allowed origins (`PATCH /v1/keys/{id}`):
 * a list of 1 to 50, or every origin, chosen on purpose and warned about. */
function OriginsDialog(props: {
  apiKey: Key;
  onClose: () => void;
  onSaved: (key: Key) => void;
  /** The key changed elsewhere (revoked): the list is read again. */
  onGone: () => void;
}) {
  const signal = componentSignal();
  const [every, setEvery] = createSignal(props.apiKey.allowed_origins === null);
  const [text, setText] = createSignal((props.apiKey.allowed_origins ?? []).join("\n"));
  const [error, setError] = createSignal<string | null>(null);
  const [failure, setFailure] = createSignal<{ text: string; detail?: string } | null>(null);
  const [busy, setBusy] = createSignal(false);
  const check = () => {
    const problem = originsProblem(every(), text());
    setError(problem);
    return problem === null;
  };
  const opening = () => every() && props.apiKey.allowed_origins !== null;

  const submit = async (e: SubmitEvent) => {
    e.preventDefault();
    if (!check() || busy()) return;
    setBusy(true);
    setFailure(null);
    try {
      const body = { allowed_origins: every() ? null : parseOrigins(text()) };
      props.onSaved(
        checkKey(await call("updateKeyOrigins", { path: { id: props.apiKey.id }, body }, signal)),
      );
    } catch (err) {
      const e2 = asApiError(err);
      if (signal.aborted || e2.sessionLost) return;
      const refusal = originsRefusal(e2);
      if (refusal?.field === true) {
        setError(refusal.message);
      } else {
        if (refusal !== null) props.onGone();
        if (e2.code === "forbidden") void loadSession();
        const detail = reference(e2);
        const shown = refusal?.message ?? describe(e2);
        setFailure(detail === undefined ? { text: shown } : { text: shown, detail });
      }
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog
      open
      heading
      title={msg("keys.originsTitle", { name: props.apiKey.name })}
      onClose={props.onClose}
    >
      <form class="space-y-4 px-5 pt-2 pb-5" noValidate onSubmit={(e) => void submit(e)}>
        <p class="text-base text-muted">
          {msg("keys.originsIntro", { prefix: `${props.apiKey.prefix}…` })}
        </p>
        <fieldset class="space-y-2">
          <legend class="text-sm font-medium text-fg">{msg("keys.originsChoice")}</legend>
          <For each={[false, true]}>
            {(all) => (
              <label class="flex items-start gap-2 text-base text-fg">
                <input
                  type="radio"
                  name="origins-scope"
                  class="mt-1 size-4 accent-accent-fill"
                  checked={every() === all}
                  onChange={() => {
                    setEvery(all);
                    setError(null);
                  }}
                />
                <span>{msg(all ? "keys.originsEvery" : "keys.originsListed")}</span>
              </label>
            )}
          </For>
        </fieldset>
        <Show
          when={!every()}
          fallback={
            <div role="alert" class="rounded-md border border-warning bg-warning-soft px-3 py-2">
              <p class="text-base font-medium text-warning">{msg("keys.originsWarnTitle")}</p>
              <p class="mt-1 text-sm text-fg">{msg("keys.originsWarn")}</p>
            </div>
          }
        >
          <TextAreaField
            label={msg("keys.origins")}
            hint={msg("keys.originsEditHint")}
            placeholder="https://tickets.example.com"
            value={text()}
            onInput={(v) => {
              setText(v);
              if (error() !== null) check();
            }}
            onBlur={check}
            error={error()}
          />
        </Show>
        <FormError message={failure()?.text ?? null} detail={failure()?.detail} />
        <div class="flex justify-end gap-2">
          <Button class="h-8" onClick={props.onClose}>
            {msg("action.cancel")}
          </Button>
          <Button
            type="submit"
            variant={opening() ? "danger" : "primary"}
            class="h-8"
            disabled={busy()}
          >
            {busy()
              ? msg("keys.originsSaving")
              : opening()
                ? msg("keys.originsOpen")
                : msg("keys.originsSave")}
          </Button>
        </div>
      </form>
    </Dialog>
  );
}

function QuickStart() {
  const [pasted, setPasted] = createSignal("");
  const kind = () => pastedKey(pasted());
  const list = createMemo(() => {
    const ws = workspace();
    return snippets({
      origin: window.location.origin,
      workspace: { key: ws?.key ?? "", name: ws?.name ?? "" },
      publishable: kind() === "publishable" ? pasted().trim() : null,
    });
  });
  return (
    <div class="max-w-3xl space-y-6">
      <p class="text-base text-muted">
        {msg("quick.intro", { origin: window.location.origin, workspace: workspace()?.name ?? "" })}
      </p>
      <div class="space-y-1">
        <label for="quick-pk" class="block text-sm font-medium text-fg">
          {msg("quick.pasteLabel")}
        </label>
        <input
          id="quick-pk"
          class={`${fieldInputClass} font-mono`}
          autocomplete="off"
          spellcheck={false}
          placeholder="pk_…"
          value={pasted()}
          aria-invalid={kind() === "secret" || kind() === "other" ? true : undefined}
          aria-describedby="quick-pk-hint"
          onInput={(e) => setPasted(e.currentTarget.value)}
        />
        <p
          id="quick-pk-hint"
          class="text-sm"
          classList={{
            "text-danger": kind() === "secret" || kind() === "other",
            "text-muted": kind() === "empty" || kind() === "publishable",
          }}
        >
          {kind() === "secret"
            ? msg("quick.pastedSecret")
            : kind() === "other"
              ? msg("quick.pastedOther")
              : msg("quick.pasteHint")}
        </p>
      </div>
      <For each={list()}>{(s) => <SnippetBlock snippet={s} />}</For>
    </div>
  );
}

function SnippetBlock(props: { snippet: Snippet }) {
  const [copied, setCopied] = createSignal(false);
  return (
    <section>
      <div class="flex items-center gap-2">
        <h2 class="mr-auto text-base font-semibold text-fg">{msg(`quick.${props.snippet.id}`)}</h2>
        <Button
          class="h-7"
          icon={copied() ? "check" : "file"}
          onClick={() =>
            void navigator.clipboard.writeText(props.snippet.code).then(
              () => setCopied(true),
              () => notify(msg("copy.failed"), "error"),
            )
          }
        >
          {copied() ? msg("copy.copied") : msg("copy.copy")}
        </Button>
      </div>
      <pre
        tabIndex={0}
        aria-label={msg(`quick.${props.snippet.id}`)}
        class="mt-2 overflow-x-auto rounded-md border border-line bg-canvas p-3 font-mono text-sm text-fg"
      >
        <code>{props.snippet.code}</code>
      </pre>
    </section>
  );
}
