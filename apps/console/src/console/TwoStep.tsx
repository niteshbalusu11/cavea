// Two-step sign-in on the account page (CON-003, D-89): whether it is on,
// turning it on (the password; then the QR code and the key as text, and a
// code from the app; then the recovery codes, shown once) and turning it
// off (the password and a code). Every secret here (the key, its URI, the
// recovery codes, the password) lives in the open dialog's memory only and
// is dropped when it closes; the clipboard is written only on a click.
import {
  createEffect,
  createSignal,
  For,
  Match,
  on,
  onCleanup,
  onMount,
  Show,
  Switch,
} from "solid-js";
import type { ConsoleTotpSecret } from "../api/generated.ts";
import { asApiError, componentSignal } from "../api/query.ts";
import { msg } from "../i18n/messages.ts";
import { adoptSecondFactor, confirmTotp, disableTotp, me, startTotp } from "../session.ts";
import { Button, buttonClass } from "../ui/controls.tsx";
import { Field, FormError } from "../ui/form.tsx";
import { Icon } from "../ui/Icon.tsx";
import { Dialog, notify } from "../ui/overlays.tsx";
import { describe, reference } from "./errors.ts";
import { CheckField, CopyField } from "./kit.tsx";
import { encodeQr, qrPath } from "./qr.ts";
import { totpRefusal } from "./refusals.ts";
import { codeProblem } from "./signInFlow.ts";
import { createWait } from "./wait.ts";

type Failure = { text: string; detail?: string } | null;

const failureOf = (e: ReturnType<typeof asApiError>): Failure => {
  const detail = reference(e);
  return detail === undefined ? { text: describe(e) } : { text: describe(e), detail };
};

/** Whether signing in asks for a code. */
const isOn = () => me()?.user.second_factor === true;
const myEmail = () => me()?.user.email ?? "";

export function TwoStep() {
  const [dialog, setDialog] = createSignal<"on" | "off" | null>(null);
  return (
    <section class="mt-10 max-w-lg" aria-labelledby="two-step-heading">
      <h2 id="two-step-heading" class="text-lg font-semibold text-fg">
        {msg("account.twoStepTitle")}
      </h2>
      <p class="mt-1 text-base text-muted">
        {isOn() ? msg("account.twoStepOn") : msg("account.twoStepOff")}
      </p>
      <Show when={isOn()}>
        <p class="mt-1 text-base text-muted">{msg("account.twoStepNewCodes")}</p>
      </Show>
      <Button
        class="mt-4 h-8"
        variant={isOn() ? "secondary" : "primary"}
        onClick={() => setDialog(isOn() ? "off" : "on")}
      >
        {isOn() ? msg("account.twoStepTurnOff") : msg("account.twoStepTurnOn")}
      </Button>
      <Show when={dialog() === "on"}>
        <TurnOn onClose={() => setDialog(null)} />
      </Show>
      <Show when={dialog() === "off"}>
        <TurnOff onClose={() => setDialog(null)} />
      </Show>
    </section>
  );
}

/** The QR code of `text` as crisp squares with a four-module quiet zone,
 * dark on light in both themes; `null` when it cannot be drawn. */
function QrImage(props: { text: string; label: string }) {
  const code = () => encodeQr(props.text);
  return (
    <Show when={code()} fallback={<p class="text-base text-warning">{msg("totp.qrTooLong")}</p>}>
      {(c) => {
        const side = () => c().size + 8;
        // Whole pixels per module, so no edge is blurred.
        const scale = () => Math.max(3, Math.floor(200 / side()));
        return (
          <svg
            role="img"
            aria-label={props.label}
            viewBox={`0 0 ${side()} ${side()}`}
            width={side() * scale()}
            height={side() * scale()}
            shape-rendering="crispEdges"
            class="block rounded-sm"
          >
            <rect width={side()} height={side()} class="fill-qr-light" />
            <path d={qrPath(c(), 4)} class="fill-qr-dark" />
          </svg>
        );
      }}
    </Show>
  );
}

/** Groups of four, as authenticator apps show a key for typing. */
const grouped = (key: string) => key.replace(/=+$/, "").replace(/(.{4})(?=.)/g, "$1 ");

function TurnOn(props: { onClose: () => void }) {
  const signal = componentSignal();
  const [stage, setStage] = createSignal<"password" | "scan" | "codes">("password");
  const [password, setPassword] = createSignal("");
  const [code, setCode] = createSignal("");
  const [secret, setSecret] = createSignal<ConsoleTotpSecret | null>(null);
  const [codes, setCodes] = createSignal<string[]>([]);
  const [errors, setErrors] = createSignal<{ password: string | null; code: string | null }>({
    password: null,
    code: null,
  });
  const [failure, setFailure] = createSignal<Failure>(null);
  const [busy, setBusy] = createSignal(false);
  const wait = createWait();
  let codeField: HTMLInputElement | undefined;
  let passwordField: HTMLInputElement | undefined;
  const forget = () => {
    setPassword("");
    setCode("");
    setSecret(null);
    setCodes([]);
  };
  onCleanup(forget);
  createEffect(
    on(
      stage,
      (now) => {
        // The codes' stage places its own focus.
        if (now !== "codes")
          queueMicrotask(() => (now === "scan" ? codeField : passwordField)?.focus());
      },
      { defer: true },
    ),
  );

  const close = () => {
    // Started, not confirmed: nothing is on, and the key is dropped.
    if (stage() === "scan") notify(msg("totp.closedEarly"));
    forget();
    props.onClose();
  };

  /** A refusal of either step: on a field, or what the page follows. */
  const refused = (err: unknown) => {
    const error = asApiError(err);
    if (signal.aborted || error.sessionLost) return;
    const refusal = totpRefusal(error);
    switch (refusal?.kind) {
      case "field":
        setErrors((x) => ({ ...x, [refusal.field]: refusal.message }));
        if (refusal.field === "password") setPassword("");
        else setCode("");
        break;
      case "alreadyOn":
        adoptSecondFactor(true);
        notify(msg("totp.alreadyOn"));
        forget();
        props.onClose();
        break;
      case "notStarted":
        // Started again elsewhere (or reset): this key is no longer the one.
        setSecret(null);
        setCode("");
        setStage("password");
        setFailure({ text: msg("totp.notStarted") });
        break;
      case "wait":
        wait.start(refusal.seconds);
        break;
      case "busy":
        setFailure({ text: msg("signIn.busy") });
        break;
      default:
        setFailure(failureOf(error));
    }
  };

  const start = async (e: SubmitEvent) => {
    e.preventDefault();
    const problem = password() === "" ? msg("totp.passwordMissing") : null;
    setErrors((x) => ({ ...x, password: problem }));
    if (problem !== null || busy() || wait.seconds() > 0) return;
    setBusy(true);
    setFailure(null);
    try {
      const s = await startTotp(password(), signal);
      setPassword("");
      setSecret(s);
      setStage("scan");
    } catch (err) {
      refused(err);
    } finally {
      setBusy(false);
    }
  };

  const confirm = async (e: SubmitEvent) => {
    e.preventDefault();
    const problem = codeProblem(code()) === null ? null : msg("totp.codeMissing");
    setErrors((x) => ({ ...x, code: problem }));
    if (problem !== null || busy() || wait.seconds() > 0) return;
    setBusy(true);
    setFailure(null);
    try {
      const list = await confirmTotp(code(), signal);
      setCode("");
      setSecret(null);
      setCodes(list);
      setStage("codes");
    } catch (err) {
      refused(err);
    } finally {
      setBusy(false);
    }
  };

  const waiting = () => (
    <Show when={wait.seconds() > 0}>
      <p role="status" class="text-base text-warning">
        {msg("signIn.wait", { seconds: wait.seconds() })}
      </p>
    </Show>
  );

  // One dialog through the three stages, so focus stays inside it; leaving
  // the codes asks first (`RecoveryCodes`).
  let leaveCodes = close;
  return (
    <Dialog
      open
      title={stage() === "codes" ? msg("codes.title") : msg("totp.title")}
      heading
      onClose={() => (stage() === "codes" ? leaveCodes() : close())}
      class="max-w-lg"
    >
      <Switch>
        <Match when={stage() === "codes"}>
          <RecoveryCodes
            codes={codes()}
            onClose={close}
            bind={(attempt) => (leaveCodes = attempt)}
          />
        </Match>
        <Match when={stage() === "password"}>
          <form class="space-y-4 px-5 pt-2 pb-5" noValidate onSubmit={(e) => void start(e)}>
            <p class="text-base text-muted">{msg("totp.passwordBody")}</p>
            <Field
              label={msg("totp.password")}
              type="password"
              autocomplete="current-password"
              value={password()}
              ref={(el) => (passwordField = el)}
              onInput={(v) => {
                setPassword(v);
                setErrors((x) => ({ ...x, password: null }));
              }}
              error={errors().password}
            />
            <FormError message={failure()?.text ?? null} detail={failure()?.detail} />
            {waiting()}
            <div class="flex justify-end gap-2">
              <Button class="h-8" onClick={close}>
                {msg("action.cancel")}
              </Button>
              <Button
                type="submit"
                variant="primary"
                class="h-8"
                disabled={busy() || wait.seconds() > 0}
              >
                {busy() ? msg("totp.starting") : msg("totp.start")}
              </Button>
            </div>
          </form>
        </Match>
        <Match when={secret()}>
          {(s) => (
            <form
              class="space-y-4 overflow-y-auto px-5 pt-2 pb-5"
              noValidate
              onSubmit={(e) => void confirm(e)}
            >
              <p class="text-base text-muted">{msg("totp.scanBody")}</p>
              <div class="flex justify-center">
                <QrImage text={s().uri} label={msg("totp.qrLabel")} />
              </div>
              <div>
                <p class="mb-2 text-sm font-medium text-fg">{msg("totp.manual")}</p>
                <CopyField label={msg("totp.secret")} value={grouped(s().secret)} />
                <p class="mt-1 text-sm text-muted">
                  {msg("totp.details", {
                    digits: s().digits,
                    period: s().period,
                    algorithm: s().algorithm,
                  })}
                </p>
              </div>
              <Field
                label={msg("totp.code")}
                autocomplete="one-time-code"
                inputMode="numeric"
                mono
                maxLength={16}
                value={code()}
                ref={(el) => (codeField = el)}
                onInput={(v) => {
                  setCode(v);
                  setErrors((x) => ({ ...x, code: null }));
                }}
                error={errors().code}
              />
              <FormError message={failure()?.text ?? null} detail={failure()?.detail} />
              {waiting()}
              <div class="flex justify-end gap-2">
                <Button class="h-8" onClick={close}>
                  {msg("action.cancel")}
                </Button>
                <Button
                  type="submit"
                  variant="primary"
                  class="h-8"
                  disabled={busy() || wait.seconds() > 0}
                >
                  {busy() ? msg("totp.confirming") : msg("totp.confirm")}
                </Button>
              </div>
            </form>
          )}
        </Match>
      </Switch>
    </Dialog>
  );
}

/** The recovery codes as a text file's contents. */
export function codesFile(email: string, codes: readonly string[], made: Date): string {
  const date = new Intl.DateTimeFormat(undefined, { dateStyle: "long" }).format(made);
  return [
    msg("codes.fileTitle", { email }),
    msg("codes.fileBody", { date }),
    "",
    ...codes,
    "",
  ].join("\n");
}

/**
 * The recovery codes, shown once. Escape, a press outside or Close without
 * the box ticked does not close it at once: it asks first, with the
 * focus on that question, so the codes are not lost by accident, and a
 * second Escape (or "Close without saving") still leaves by keyboard.
 */
function RecoveryCodes(props: {
  codes: string[];
  onClose: () => void;
  /** Gives the dialog what Escape and a press outside do. */
  bind: (attempt: () => void) => void;
}) {
  const [saved, setSaved] = createSignal(false);
  const [asking, setAsking] = createSignal(false);
  const [copied, setCopied] = createSignal<"no" | "yes" | "failed">("no");
  let leave: HTMLButtonElement | undefined;
  let first: HTMLButtonElement | undefined;
  let list: HTMLOListElement | undefined;
  const attempt = () => {
    if (saved() || asking()) {
      props.onClose();
      return;
    }
    setAsking(true);
    queueMicrotask(() => leave?.focus());
  };
  props.bind(attempt);
  onMount(() => queueMicrotask(() => first?.focus()));
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(props.codes.join("\n"));
      setCopied("yes");
    } catch {
      // Selected for the person to copy themselves.
      const range = document.createRange();
      if (list !== undefined) range.selectNodeContents(list);
      const selection = window.getSelection();
      selection?.removeAllRanges();
      selection?.addRange(range);
      setCopied("failed");
    }
  };
  const download = () => {
    const blob = new Blob([codesFile(myEmail(), props.codes, new Date())], {
      type: "text/plain;charset=utf-8",
    });
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url;
    a.download = "cavea-recovery-codes.txt";
    a.click();
    setTimeout(() => URL.revokeObjectURL(url), 0);
  };
  return (
    <div class="space-y-4 overflow-y-auto px-5 pt-2 pb-5">
      <p role="status" class="text-base text-success">
        {msg("codes.turnedOn")}
      </p>
      <p class="text-base text-muted">{msg("codes.body")}</p>
      <div data-print class="rounded-md border border-line bg-surface p-3">
        <p class="mb-2 hidden text-base print:block">
          {msg("codes.fileTitle", { email: myEmail() })}
        </p>
        <ol
          ref={(el) => (list = el)}
          aria-label={msg("codes.list")}
          class="grid grid-cols-2 gap-x-6 gap-y-1 font-mono text-base text-fg"
        >
          <For each={props.codes}>{(c) => <li>{c}</li>}</For>
        </ol>
      </div>
      <div class="flex flex-wrap gap-2">
        <button
          type="button"
          ref={(el) => (first = el)}
          class={buttonClass("secondary", "h-8")}
          onClick={() => void copy()}
        >
          <Icon name={copied() === "yes" ? "check" : "file"} />
          {copied() === "yes" ? msg("codes.copied") : msg("codes.copy")}
        </button>
        <Button class="h-8" icon="download" onClick={download}>
          {msg("codes.download")}
        </Button>
        <Button class="h-8" icon="report" onClick={() => window.print()}>
          {msg("codes.print")}
        </Button>
      </div>
      <p role="status" class="text-sm text-muted">
        {copied() === "failed" ? msg("codes.copyFailed") : ""}
      </p>
      <CheckField
        label={msg("codes.saved")}
        checked={saved()}
        onChange={(v) => {
          setSaved(v);
          if (v) setAsking(false);
        }}
      />
      <Show when={asking()}>
        <div role="alert" class="rounded-md bg-warning-soft px-3 py-2 text-base text-fg">
          <p>{msg("codes.leaveWarning")}</p>
          <button
            type="button"
            ref={(el) => (leave = el)}
            class={buttonClass("secondary", "mt-2 h-8")}
            onClick={() => props.onClose()}
          >
            {msg("codes.leave")}
          </button>
        </div>
      </Show>
      <div class="flex justify-end">
        <Button variant="primary" class="h-8" onClick={attempt}>
          {msg("codes.done")}
        </Button>
      </div>
    </div>
  );
}

function TurnOff(props: { onClose: () => void }) {
  const signal = componentSignal();
  const [password, setPassword] = createSignal("");
  const [code, setCode] = createSignal("");
  const [errors, setErrors] = createSignal<{ password: string | null; code: string | null }>({
    password: null,
    code: null,
  });
  const [failure, setFailure] = createSignal<Failure>(null);
  const [busy, setBusy] = createSignal(false);
  const wait = createWait();
  const forget = () => {
    setPassword("");
    setCode("");
  };
  onCleanup(forget);
  const close = () => {
    forget();
    props.onClose();
  };

  const submit = async (e: SubmitEvent) => {
    e.preventDefault();
    const problems = {
      password: password() === "" ? msg("totp.passwordMissing") : null,
      code: codeProblem(code()),
    };
    setErrors(problems);
    if (problems.password !== null || problems.code !== null) return;
    if (busy() || wait.seconds() > 0) return;
    setBusy(true);
    setFailure(null);
    try {
      await disableTotp(password(), code(), signal);
      notify(msg("totpOff.done"));
      close();
    } catch (err) {
      const error = asApiError(err);
      if (signal.aborted || error.sessionLost) return;
      const refusal = totpRefusal(error, true);
      switch (refusal?.kind) {
        case "field":
          setErrors((x) => ({ ...x, [refusal.field]: refusal.message }));
          if (refusal.field === "password") setPassword("");
          else setCode("");
          break;
        case "notOn":
          adoptSecondFactor(false);
          notify(msg("totpOff.notOn"));
          close();
          break;
        case "wait":
          wait.start(refusal.seconds);
          break;
        case "busy":
          setFailure({ text: msg("signIn.busy") });
          break;
        default:
          setFailure(failureOf(error));
      }
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog open title={msg("totpOff.title")} heading onClose={close}>
      <form class="space-y-4 px-5 pt-2 pb-5" noValidate onSubmit={(e) => void submit(e)}>
        <p class="text-base text-muted">{msg("totpOff.body")}</p>
        <Field
          label={msg("totp.password")}
          type="password"
          autocomplete="current-password"
          value={password()}
          onInput={(v) => {
            setPassword(v);
            setErrors((x) => ({ ...x, password: null }));
          }}
          error={errors().password}
        />
        <Field
          label={msg("totpOff.code")}
          autocomplete="one-time-code"
          mono
          maxLength={64}
          hint={msg("signIn.codeHint")}
          value={code()}
          onInput={(v) => {
            setCode(v);
            setErrors((x) => ({ ...x, code: null }));
          }}
          error={errors().code}
        />
        <FormError message={failure()?.text ?? null} detail={failure()?.detail} />
        <Show when={wait.seconds() > 0}>
          <p role="status" class="text-base text-warning">
            {msg("signIn.wait", { seconds: wait.seconds() })}
          </p>
        </Show>
        <div class="flex justify-end gap-2">
          <Button class="h-8" onClick={close}>
            {msg("action.cancel")}
          </Button>
          <Button
            type="submit"
            variant="danger"
            class="h-8"
            disabled={busy() || wait.seconds() > 0}
          >
            {busy() ? msg("totpOff.submitting") : msg("totpOff.submit")}
          </Button>
        </div>
      </form>
    </Dialog>
  );
}
