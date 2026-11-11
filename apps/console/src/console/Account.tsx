// The signed-in person's account (CON-003): who they are and a password
// change. A change ends every other session of theirs; this one continues
// with a new cookie and CSRF token.
import { createSignal, onCleanup, Show } from "solid-js";
import { msg } from "../i18n/messages.ts";
import { asApiError, componentSignal } from "../api/query.ts";
import { changePassword, me } from "../session.ts";
import { Button } from "../ui/controls.tsx";
import { Field, FormError } from "../ui/form.tsx";
import { notify } from "../ui/overlays.tsx";
import { describe, reference } from "./errors.ts";
import { passwordProblem } from "./Setup.tsx";
import { createWait } from "./wait.ts";

type FieldName = "current" | "next" | "confirm";

export function Account() {
  const signal = componentSignal();
  const [values, setValues] = createSignal<Record<FieldName, string>>({
    current: "",
    next: "",
    confirm: "",
  });
  const [errors, setErrors] = createSignal<Record<FieldName, string | null>>({
    current: null,
    next: null,
    confirm: null,
  });
  const [failure, setFailure] = createSignal<{ text: string; detail?: string } | null>(null);
  const [busy, setBusy] = createSignal(false);
  const wait = createWait();
  const clear = () => setValues({ current: "", next: "", confirm: "" });
  onCleanup(clear);

  const check = (field: FieldName): boolean => {
    const v = values();
    const problem =
      field === "current"
        ? v.current === ""
          ? msg("account.currentMissing")
          : null
        : field === "next"
          ? passwordProblem(v.next)
          : v.confirm !== v.next
            ? msg("password.mismatch")
            : null;
    setErrors((e) => ({ ...e, [field]: problem }));
    return problem === null;
  };
  const input = (field: FieldName) => (value: string) => {
    setValues((v) => ({ ...v, [field]: value }));
    if (errors()[field] !== null) check(field);
  };

  const submit = async (e: SubmitEvent) => {
    e.preventDefault();
    const ok = [check("current"), check("next"), check("confirm")].every(Boolean);
    if (!ok || busy() || wait.seconds() > 0) return;
    setBusy(true);
    setFailure(null);
    try {
      await changePassword(values().current, values().next, signal);
      clear();
      notify(msg("account.changed"));
    } catch (err) {
      const error = asApiError(err);
      if (signal.aborted || error.sessionLost) return;
      if (error.status === 403 && error.reason === "credentials") {
        setErrors((x) => ({ ...x, current: msg("account.currentWrong") }));
        setValues((v) => ({ ...v, current: "" }));
      } else if (error.reason === "invalid_field") {
        setErrors((x) => ({ ...x, next: msg("password.rule") }));
      } else if (error.status === 429) {
        wait.start(error.retryAfter ?? 60);
      } else {
        const detail = reference(error);
        setFailure(
          detail === undefined ? { text: describe(error) } : { text: describe(error), detail },
        );
      }
    } finally {
      setBusy(false);
    }
  };

  return (
    <>
      <h1 class="text-xl font-semibold text-fg">{msg("account.title")}</h1>
      <p class="mt-1 text-base text-muted">
        {msg("account.signedInAs", { email: me()?.user.email ?? "" })}
      </p>
      <section class="mt-6 max-w-sm" aria-labelledby="password-heading">
        <h2 id="password-heading" class="text-lg font-semibold text-fg">
          {msg("account.passwordTitle")}
        </h2>
        <p class="mt-1 mb-4 text-base text-muted">{msg("account.passwordBody")}</p>
        <form class="space-y-4" noValidate onSubmit={(e) => void submit(e)}>
          <Field
            label={msg("account.current")}
            type="password"
            autocomplete="current-password"
            value={values().current}
            onInput={input("current")}
            onBlur={() => check("current")}
            error={errors().current}
          />
          <Field
            label={msg("account.next")}
            type="password"
            autocomplete="new-password"
            hint={msg("password.rule")}
            value={values().next}
            onInput={input("next")}
            onBlur={() => check("next")}
            error={errors().next}
          />
          <Field
            label={msg("account.confirm")}
            type="password"
            autocomplete="new-password"
            value={values().confirm}
            onInput={input("confirm")}
            onBlur={() => check("confirm")}
            error={errors().confirm}
          />
          <FormError message={failure()?.text ?? null} detail={failure()?.detail} />
          <Show when={wait.seconds() > 0}>
            <p role="status" class="text-base text-warning">
              {msg("signIn.wait", { seconds: wait.seconds() })}
            </p>
          </Show>
          <Button
            type="submit"
            variant="primary"
            class="h-8"
            disabled={busy() || wait.seconds() > 0}
          >
            {busy() ? msg("account.submitting") : msg("account.submit")}
          </Button>
        </form>
      </section>
    </>
  );
}
