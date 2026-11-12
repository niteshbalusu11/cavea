// A password reset link's page (09 §10, CON-003, D-90):
// `/console/reset#token=…`. The token left the address bar at start
// (resetToken.ts) and lives in memory until it is used or refused. The
// person chooses a new password; saving it ends every earlier session of
// theirs and signs them in, or, when their account still has a second
// factor, asks for a code as sign-in does (`CodeStep.tsx`).
import { A, useNavigate } from "@solidjs/router";
import { createEffect, createSignal, Match, on, onCleanup, Show, Switch } from "solid-js";
import { asApiError, componentSignal } from "../api/query.ts";
import { msg } from "../i18n/messages.ts";
import { me, session, redeemResetLink } from "../session.ts";
import { Button } from "../ui/controls.tsx";
import { Card, Field, FormError } from "../ui/form.tsx";
import { notify } from "../ui/overlays.tsx";
import { CodeStep } from "./CodeStep.tsx";
import { describe, reference } from "./errors.ts";
import { resetRefusal } from "./refusals.ts";
import { dropResetToken, resetToken } from "./resetToken.ts";
import { passwordProblem } from "./Setup.tsx";
import { next as advance, START, type Flow, type FlowEvent } from "./signInFlow.ts";
import { Failed, Loading } from "./states.tsx";
import { createWait } from "./wait.ts";

/** Where the page stands once the link is no longer usable. */
type Ended = "spent" | "codeEnded" | null;

export function Reset() {
  const [token, setToken] = createSignal(resetToken());
  const [ended, setEnded] = createSignal<Ended>(null);
  const end = (why: Exclude<Ended, null>) => {
    dropResetToken();
    setToken(null);
    setEnded(why);
  };
  return (
    <Switch>
      <Match when={ended()}>
        {(why) => (
          <Card title={msg("reset.title")}>
            <p role="alert" class="text-base text-fg">
              {why() === "spent" ? msg("reset.spent") : msg("reset.challengeEnded")}
            </p>
            <A href="/sign-in" class="mt-4 inline-block text-base text-accent underline">
              {msg("reset.toSignIn")}
            </A>
          </Card>
        )}
      </Match>
      <Match when={token() === null}>
        <Card title={msg("reset.title")}>
          <p class="text-base text-fg">{msg("reset.noToken")}</p>
          <A href="/sign-in" class="mt-4 inline-block text-base text-accent underline">
            {msg("reset.toSignIn")}
          </A>
        </Card>
      </Match>
      <Match when={token() === "malformed"}>
        <Card title={msg("reset.title")}>
          <p role="alert" class="text-base text-fg">
            {msg("reset.malformed")}
          </p>
        </Card>
      </Match>
      <Match when={session().kind === "loading"}>
        <Loading />
      </Match>
      <Match when={session().kind === "failed"}>
        <Failed />
      </Match>
      <Match
        when={(() => {
          const t = token();
          return t !== null && t !== "malformed" ? t : null;
        })()}
      >
        {(t) => <ResetForm token={t()} onEnded={end} />}
      </Match>
    </Switch>
  );
}

function ResetForm(props: { token: string; onEnded: (why: "spent" | "codeEnded") => void }) {
  const signal = componentSignal();
  const navigate = useNavigate();
  // Who was signed in when the page opened: saving signs in as the link's
  // person instead, and the page says so.
  const before = me()?.user.email ?? null;
  const [password, setPassword] = createSignal("");
  const [confirm, setConfirm] = createSignal("");
  const [errors, setErrors] = createSignal<{ password: string | null; confirm: string | null }>({
    password: null,
    confirm: null,
  });
  const [failure, setFailure] = createSignal<{ text: string; detail?: string } | null>(null);
  const [flow, setFlow] = createSignal<Flow>(START);
  const dispatch = (event: FlowEvent) => setFlow((f) => advance(f, event));
  const [busy, setBusy] = createSignal(false);
  const wait = createWait();
  onCleanup(() => {
    setPassword("");
    setConfirm("");
    setFlow(START);
  });

  const done = () => {
    dropResetToken();
    notify(msg("reset.done"));
    navigate("/charts", { replace: true });
  };
  // A challenge that died: the password is saved and the link used, so
  // the person signs in as usual.
  createEffect(
    on(
      () => flow(),
      (now, was) => {
        if (now.step === "password" && was?.step === "code") props.onEnded("codeEnded");
      },
    ),
  );

  const checkPassword = () => {
    const p = passwordProblem(password());
    setErrors((e) => ({ ...e, password: p }));
    return p === null;
  };
  const checkConfirm = () => {
    const p = confirm() === password() ? null : msg("password.mismatch");
    setErrors((e) => ({ ...e, confirm: p }));
    return p === null;
  };

  const submit = async (e: SubmitEvent) => {
    e.preventDefault();
    if (![checkPassword(), checkConfirm()].every(Boolean)) return;
    if (busy() || wait.seconds() > 0) return;
    setBusy(true);
    setFailure(null);
    try {
      const signed = await redeemResetLink(props.token, password(), signal);
      setPassword("");
      setConfirm("");
      // The link is used either way.
      dropResetToken();
      if (signed.kind === "code") {
        dispatch({ type: "challenge", challenge: signed.challenge });
      } else {
        done();
      }
    } catch (err) {
      const error = asApiError(err);
      if (signal.aborted) return;
      const refusal = resetRefusal(error);
      switch (refusal?.kind) {
        case "spent":
          props.onEnded("spent");
          break;
        case "password":
          setErrors((x) => ({ ...x, password: msg("password.rule") }));
          break;
        case "wait":
          wait.start(refusal.seconds);
          break;
        case "busy":
          setFailure({ text: msg("signIn.busy") });
          break;
        default: {
          const detail = reference(error);
          setFailure(
            detail === undefined ? { text: describe(error) } : { text: describe(error), detail },
          );
        }
      }
    } finally {
      setBusy(false);
    }
  };

  return (
    <Card title={msg("reset.title")}>
      <Show
        when={flow().step === "code"}
        fallback={
          <form class="space-y-4" noValidate onSubmit={(e) => void submit(e)}>
            <p class="text-base text-muted">{msg("reset.body")}</p>
            <Show when={before}>
              {(email) => (
                <p class="rounded-md bg-warning-soft px-3 py-2 text-base text-fg">
                  {msg("reset.signedInNote", { email: email() })}
                </p>
              )}
            </Show>
            <Field
              label={msg("reset.password")}
              type="password"
              autocomplete="new-password"
              hint={msg("password.rule")}
              value={password()}
              onInput={(v) => {
                setPassword(v);
                if (errors().password !== null) checkPassword();
              }}
              onBlur={checkPassword}
              error={errors().password}
            />
            <Field
              label={msg("reset.confirm")}
              type="password"
              autocomplete="new-password"
              value={confirm()}
              onInput={(v) => {
                setConfirm(v);
                if (errors().confirm !== null) checkConfirm();
              }}
              onBlur={checkConfirm}
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
              {busy() ? msg("reset.submitting") : msg("reset.submit")}
            </Button>
          </form>
        }
      >
        <CodeStep
          flow={flow()}
          dispatch={dispatch}
          body={msg("reset.codeBody")}
          restart={msg("reset.toSignIn")}
          onRestart={() => navigate("/sign-in")}
          onSignedIn={done}
        />
      </Show>
    </Card>
  );
}
