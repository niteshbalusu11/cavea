// Sign-in (09 §2, CON-003). A wrong email and a wrong password read alike,
// as the server answers them; a back-off or rate limit waits out its
// `Retry-After` with the button disabled. Signed in, the person goes back
// to where they were (`?next=`).
import { Navigate, useSearchParams } from "@solidjs/router";
import { createSignal, Match, onCleanup, Show, Switch } from "solid-js";
import { msg } from "../i18n/messages.ts";
import { asApiError, componentSignal } from "../api/query.ts";
import { session, signIn } from "../session.ts";
import { Button } from "../ui/controls.tsx";
import { Card, Field, FormError } from "../ui/form.tsx";
import { describe, reference } from "./errors.ts";
import { safeNext } from "./paths.ts";
import { Failed, Loading } from "./states.tsx";
import { createWait } from "./wait.ts";

export function SignIn() {
  const [params] = useSearchParams<{ next?: string }>();
  const next = () => safeNext(params.next);
  return (
    <Switch>
      <Match when={session().kind === "signedIn"}>
        <Navigate href={next()} />
      </Match>
      <Match when={session().kind === "loading"}>
        <Loading />
      </Match>
      <Match when={session().kind === "failed"}>
        <Failed />
      </Match>
      <Match when={session().kind === "signedOut"}>
        <SignInForm />
      </Match>
    </Switch>
  );
}

const ended = () => {
  const s = session();
  return s.kind === "signedOut" && s.cause === "ended";
};

function SignInForm() {
  const signal = componentSignal();
  const [email, setEmail] = createSignal("");
  const [password, setPassword] = createSignal("");
  const [emailError, setEmailError] = createSignal<string | null>(null);
  const [passwordError, setPasswordError] = createSignal<string | null>(null);
  const [failure, setFailure] = createSignal<{ text: string; detail?: string } | null>(null);
  const [busy, setBusy] = createSignal(false);
  const wait = createWait();
  onCleanup(() => setPassword(""));

  const checkEmail = () => setEmailError(email().trim() === "" ? msg("signIn.emailMissing") : null);
  const checkPassword = () =>
    setPasswordError(password() === "" ? msg("signIn.passwordMissing") : null);

  const submit = async (e: SubmitEvent) => {
    e.preventDefault();
    checkEmail();
    checkPassword();
    if (emailError() !== null || passwordError() !== null || busy() || wait.seconds() > 0) return;
    setBusy(true);
    setFailure(null);
    try {
      await signIn(email().trim(), password(), signal);
      // The page follows the session to `next`.
    } catch (err) {
      const error = asApiError(err);
      if (signal.aborted) return;
      setPassword("");
      if (error.status === 401 && error.reason === "credentials") {
        setFailure({ text: msg("signIn.wrong") });
      } else if (error.status === 429) {
        wait.start(error.retryAfter ?? 60);
        setFailure(null);
      } else if (error.reason === "password_hashing") {
        setFailure({ text: msg("signIn.busy") });
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
    <Card title={msg("signIn.title")}>
      <Show when={ended()}>
        <p role="status" class="mb-4 rounded-md bg-warning-soft px-3 py-2 text-base text-fg">
          {msg("signIn.ended")}
        </p>
      </Show>
      <form class="space-y-4" noValidate onSubmit={(e) => void submit(e)}>
        <Field
          label={msg("signIn.email")}
          type="email"
          autocomplete="username"
          value={email()}
          onInput={(v) => {
            setEmail(v);
            if (emailError() !== null) checkEmail();
          }}
          onBlur={checkEmail}
          error={emailError()}
        />
        <Field
          label={msg("signIn.password")}
          type="password"
          autocomplete="current-password"
          value={password()}
          onInput={(v) => {
            setPassword(v);
            if (passwordError() !== null) checkPassword();
          }}
          onBlur={checkPassword}
          error={passwordError()}
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
          {busy() ? msg("signIn.busySubmitting") : msg("signIn.submit")}
        </Button>
      </form>
      <p class="mt-4 text-sm text-muted">{msg("signIn.forgot")}</p>
    </Card>
  );
}
