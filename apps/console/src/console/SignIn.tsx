// Sign-in (09 §2, CON-003). A wrong email and a wrong password read alike,
// as the server answers them; a back-off or rate limit waits out its
// `Retry-After` with the button disabled. An account with a second factor
// is asked for a code next (`CodeStep.tsx`, D-89); a challenge that ends
// sends the person back to the password with the email kept. Signed in,
// the person goes back to where they were (`?next=`, which neither step
// changes).
import { Navigate, useSearchParams } from "@solidjs/router";
import { createEffect, createSignal, Match, on, onCleanup, Show, Switch } from "solid-js";
import { msg } from "../i18n/messages.ts";
import { asApiError, componentSignal } from "../api/query.ts";
import { session, signIn } from "../session.ts";
import { Button } from "../ui/controls.tsx";
import { Card, Field, FormError } from "../ui/form.tsx";
import { CodeStep, said } from "./CodeStep.tsx";
import { safeNext } from "./paths.ts";
import { Failed, Loading } from "./states.tsx";
import { next as advance, START, type Flow, type FlowEvent } from "./signInFlow.ts";
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
  // The steps; the challenge in it is a secret, dropped with the page.
  const [flow, setFlow] = createSignal<Flow>(START);
  const dispatch = (event: FlowEvent) => setFlow((f) => advance(f, event));
  const [busy, setBusy] = createSignal(false);
  const wait = createWait();
  let passwordField: HTMLInputElement | undefined;
  onCleanup(() => {
    setPassword("");
    setFlow(START);
  });
  createEffect(
    on(
      () => flow(),
      (now, before) => {
        if (now.step === "password" && now.wait !== null) wait.start(now.wait);
        // Back from the code step: the password again, the email kept.
        if (now.step === "password" && before?.step === "code") {
          queueMicrotask(() => passwordField?.focus());
        }
      },
    ),
  );

  const checkEmail = () => setEmailError(email().trim() === "" ? msg("signIn.emailMissing") : null);
  const checkPassword = () =>
    setPasswordError(password() === "" ? msg("signIn.passwordMissing") : null);

  const submit = async (e: SubmitEvent) => {
    e.preventDefault();
    checkEmail();
    checkPassword();
    if (emailError() !== null || passwordError() !== null || busy() || wait.seconds() > 0) return;
    setBusy(true);
    dispatch({ type: "submit" });
    try {
      const signed = await signIn(email().trim(), password(), signal);
      setPassword("");
      // Signed in: the page follows the session to `next`.
      if (signed.kind === "code") dispatch({ type: "challenge", challenge: signed.challenge });
    } catch (err) {
      const error = asApiError(err);
      if (signal.aborted) return;
      setPassword("");
      dispatch({ type: "refused", error });
    } finally {
      setBusy(false);
    }
  };

  return (
    <Card title={msg("signIn.title")}>
      <Show when={ended() && flow().step === "password"}>
        <p role="status" class="mb-4 rounded-md bg-warning-soft px-3 py-2 text-base text-fg">
          {msg("signIn.ended")}
        </p>
      </Show>
      <Show
        when={flow().step === "code"}
        fallback={
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
              ref={(el) => (passwordField = el)}
              onInput={(v) => {
                setPassword(v);
                if (passwordError() !== null) checkPassword();
              }}
              onBlur={checkPassword}
              error={passwordError()}
            />
            <FormError message={said(flow()).text} detail={said(flow()).detail} />
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
        }
      >
        <CodeStep
          flow={flow()}
          dispatch={dispatch}
          body={msg("signIn.codeBody")}
          restart={msg("signIn.restart")}
          onRestart={() => dispatch({ type: "restart" })}
        />
      </Show>
      <p class="mt-4 text-sm text-muted">{msg("signIn.forgot")}</p>
    </Card>
  );
}
