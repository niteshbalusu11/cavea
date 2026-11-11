// The invitation link's page (09 §7, CON-012): `/console/invite#token=…`.
// The token left the address bar at start (inviteToken.ts). Signed in, the
// person accepts as themselves; signed out, a new person sets a password
// and lands signed in; someone with an account signs in first and comes
// back here, the token still in memory.
import { A, useLocation, useNavigate } from "@solidjs/router";
import { createSignal, Match, onCleanup, Show, Switch } from "solid-js";
import { asApiError, componentSignal } from "../api/query.ts";
import { msg } from "../i18n/messages.ts";
import { acceptInvitation, me, session, signOut } from "../session.ts";
import { Button } from "../ui/controls.tsx";
import { Card, Field, FormError } from "../ui/form.tsx";
import { describe, reference, report } from "./errors.ts";
import { dropInviteToken, inviteToken } from "./inviteToken.ts";
import { here, signInFor } from "./paths.ts";
import { acceptRefusal } from "./refusals.ts";
import { passwordProblem } from "./Setup.tsx";
import { Failed, Loading } from "./states.tsx";
import { createWait } from "./wait.ts";

export function Invite() {
  const [token, setToken] = createSignal(inviteToken());
  const spend = () => {
    dropInviteToken();
    setToken(null);
    setSpent(true);
  };
  const [spent, setSpent] = createSignal(false);
  return (
    <Switch>
      <Match when={spent()}>
        <Card title={msg("accept.title")}>
          <p role="alert" class="text-base text-fg">
            {msg("accept.spent")}
          </p>
          <A href="/charts" class="mt-4 inline-block text-base text-accent underline">
            {msg("accept.toConsole")}
          </A>
        </Card>
      </Match>
      <Match when={token() === null}>
        <Card title={msg("accept.title")}>
          <p class="text-base text-fg">{msg("accept.noToken")}</p>
        </Card>
      </Match>
      <Match when={token() === "malformed"}>
        <Card title={msg("accept.title")}>
          <p role="alert" class="text-base text-fg">
            {msg("accept.malformed")}
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
        {(t) => <Accept token={t()} onSpent={spend} />}
      </Match>
    </Switch>
  );
}

const signedIn = () => session().kind === "signedIn";

function Accept(props: { token: string; onSpent: () => void }) {
  const signal = componentSignal();
  const navigate = useNavigate();
  const location = useLocation();
  const [password, setPassword] = createSignal("");
  const [confirm, setConfirm] = createSignal("");
  const [errors, setErrors] = createSignal<{ password: string | null; confirm: string | null }>({
    password: null,
    confirm: null,
  });
  const [failure, setFailure] = createSignal<{ text: string; detail?: string } | null>(null);
  const [problem, setProblem] = createSignal<"otherEmail" | "signIn" | null>(null);
  const [busy, setBusy] = createSignal(false);
  const wait = createWait();
  onCleanup(() => {
    setPassword("");
    setConfirm("");
  });

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
    const withPassword = !signedIn();
    if (withPassword && ![checkPassword(), checkConfirm()].every(Boolean)) return;
    if (busy() || wait.seconds() > 0) return;
    setBusy(true);
    setFailure(null);
    setProblem(null);
    try {
      await acceptInvitation(props.token, withPassword ? password() : undefined, signal);
      dropInviteToken();
      navigate("/charts", { replace: true });
    } catch (err) {
      const error = asApiError(err);
      if (signal.aborted) return;
      const refusal = acceptRefusal(error);
      switch (refusal?.kind) {
        case "spent":
          props.onSpent();
          break;
        case "otherEmail":
        case "signIn":
          setPassword("");
          setConfirm("");
          setProblem(refusal.kind);
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
    <Card title={msg("accept.title")}>
      <Show when={problem() === "otherEmail"}>
        <div role="alert" class="mb-4 rounded-md bg-warning-soft px-3 py-2 text-base text-fg">
          <p>{msg("accept.otherEmail", { email: me()?.user.email ?? "" })}</p>
          <Button
            class="mt-2 h-8"
            onClick={() => {
              setProblem(null);
              signOut(signal).catch(report);
            }}
          >
            {msg("user.signOut")}
          </Button>
        </div>
      </Show>
      <Show when={problem() === "signIn"}>
        <div role="alert" class="mb-4 rounded-md bg-warning-soft px-3 py-2 text-base text-fg">
          <p>{msg("accept.hasAccount")}</p>
          <A href={signInFor(here(location))} class="mt-2 block text-base text-accent underline">
            {msg("accept.toSignIn")}
          </A>
        </div>
      </Show>
      <form class="space-y-4" noValidate onSubmit={(e) => void submit(e)}>
        <Show
          when={signedIn()}
          fallback={
            <>
              <p class="text-base text-muted">{msg("accept.newBody")}</p>
              <Field
                label={msg("setup.password")}
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
                label={msg("setup.confirm")}
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
            </>
          }
        >
          <p class="text-base text-muted">
            {msg("accept.signedInBody", { email: me()?.user.email ?? "" })}
          </p>
        </Show>
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
          {busy() ? msg("accept.submitting") : msg("accept.submit")}
        </Button>
      </form>
      <Show when={!signedIn()}>
        <p class="mt-4 text-sm text-muted">
          {msg("accept.haveAccount")}{" "}
          <A href={signInFor(here(location))} class="text-accent underline">
            {msg("accept.toSignIn")}
          </A>
        </p>
      </Show>
    </Card>
  );
}
