// First run (09 §2, CON-002): the setup link's page. The token came from
// the address bar's fragment at start and is already gone from it
// (setupToken.ts); this page posts it with the owner's email and password,
// and the owner lands signed in.
import { A, useNavigate } from "@solidjs/router";
import { createSignal, Match, onCleanup, Show, Switch } from "solid-js";
import { msg } from "../i18n/messages.ts";
import { asApiError, componentSignal } from "../api/query.ts";
import { setUp } from "../session.ts";
import { Button } from "../ui/controls.tsx";
import { Card, Field, FormError, length, PASSWORD_MAX, PASSWORD_MIN } from "../ui/form.tsx";
import { describe, reference } from "./errors.ts";
import { dropSetupToken, setupToken } from "./setupToken.ts";
import { createWait } from "./wait.ts";

export function Setup() {
  const token = setupToken();
  return (
    <Switch>
      <Match when={token === null}>
        <Card title={msg("setup.title")}>
          <p class="text-base text-fg">{msg("setup.noToken")}</p>
          <A href="/sign-in" class="mt-4 inline-block text-base text-accent underline">
            {msg("setup.toSignIn")}
          </A>
        </Card>
      </Match>
      <Match when={token === "malformed"}>
        <Card title={msg("setup.title")}>
          <p role="alert" class="text-base text-fg">
            {msg("setup.malformed")}
          </p>
        </Card>
      </Match>
      <Match when={token !== null && token !== "malformed" ? token : null}>
        {(t) => <SetupForm token={t()} />}
      </Match>
    </Switch>
  );
}

/** Password rules as the server holds them: 15 to 128 characters. */
export function passwordProblem(password: string): string | null {
  const n = length(password);
  if (n < PASSWORD_MIN) return msg("password.short", { min: PASSWORD_MIN });
  if (n > PASSWORD_MAX) return msg("password.long", { max: PASSWORD_MAX });
  return null;
}

function SetupForm(props: { token: string }) {
  const signal = componentSignal();
  const navigate = useNavigate();
  const [email, setEmail] = createSignal("");
  const [password, setPassword] = createSignal("");
  const [confirm, setConfirm] = createSignal("");
  const [errors, setErrors] = createSignal<Record<"email" | "password" | "confirm", string | null>>(
    { email: null, password: null, confirm: null },
  );
  const [failure, setFailure] = createSignal<{ text: string; detail?: string } | null>(null);
  const [spent, setSpent] = createSignal(false);
  const [busy, setBusy] = createSignal(false);
  const wait = createWait();
  onCleanup(() => {
    setPassword("");
    setConfirm("");
  });

  const check = (field: "email" | "password" | "confirm") => {
    const problem =
      field === "email"
        ? email().trim() === "" || !email().includes("@")
          ? msg("setup.emailInvalid")
          : null
        : field === "password"
          ? passwordProblem(password())
          : confirm() !== password()
            ? msg("password.mismatch")
            : null;
    setErrors((e) => ({ ...e, [field]: problem }));
    return problem === null;
  };

  const submit = async (e: SubmitEvent) => {
    e.preventDefault();
    const ok = [check("email"), check("password"), check("confirm")].every(Boolean);
    if (!ok || busy() || spent() || wait.seconds() > 0) return;
    setBusy(true);
    setFailure(null);
    try {
      await setUp(props.token, email().trim(), password(), signal);
      dropSetupToken();
      navigate("/charts", { replace: true });
    } catch (err) {
      const error = asApiError(err);
      if (signal.aborted) return;
      const field = error.details["field"];
      if (error.status === 401 && error.reason === "setup_token") {
        setSpent(true);
        dropSetupToken();
      } else if (error.reason === "invalid_field" && (field === "email" || field === "password")) {
        setErrors((x) => ({
          ...x,
          [field]: field === "email" ? msg("setup.emailInvalid") : msg("password.rule"),
        }));
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
    <Card title={msg("setup.title")}>
      <Show
        when={!spent()}
        fallback={
          <>
            <p role="alert" class="text-base text-fg">
              {msg("setup.spent")}
            </p>
            <A href="/sign-in" class="mt-4 inline-block text-base text-accent underline">
              {msg("setup.toSignIn")}
            </A>
          </>
        }
      >
        <p class="mb-4 text-base text-muted">{msg("setup.body")}</p>
        <form class="space-y-4" noValidate onSubmit={(e) => void submit(e)}>
          <Field
            label={msg("setup.email")}
            type="email"
            autocomplete="username"
            value={email()}
            onInput={(v) => {
              setEmail(v);
              if (errors().email !== null) check("email");
            }}
            onBlur={() => check("email")}
            error={errors().email}
          />
          <Field
            label={msg("setup.password")}
            type="password"
            autocomplete="new-password"
            hint={msg("password.rule")}
            value={password()}
            onInput={(v) => {
              setPassword(v);
              if (errors().password !== null) check("password");
            }}
            onBlur={() => check("password")}
            error={errors().password}
          />
          <Field
            label={msg("setup.confirm")}
            type="password"
            autocomplete="new-password"
            value={confirm()}
            onInput={(v) => {
              setConfirm(v);
              if (errors().confirm !== null) check("confirm");
            }}
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
            class="h-8 w-full"
            disabled={busy() || wait.seconds() > 0}
          >
            {busy() ? msg("setup.submitting") : msg("setup.submit")}
          </Button>
        </form>
      </Show>
    </Card>
  );
}
