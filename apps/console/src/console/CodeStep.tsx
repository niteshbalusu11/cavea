// Sign-in's second step (CON-003, D-89), on the sign-in page and on a reset
// link's page alike: one field for the six digits from the app or a
// recovery code, posted with the challenge the password step gave. The
// rules are `signInFlow.ts`'s; this is their form. The challenge and the
// code live in this page's memory only.
import { createEffect, createSignal, on, onCleanup, onMount, Show } from "solid-js";
import { asApiError, componentSignal } from "../api/query.ts";
import { msg } from "../i18n/messages.ts";
import { signInWithCode } from "../session.ts";
import { Button } from "../ui/controls.tsx";
import { Field, FormError } from "../ui/form.tsx";
import { describe, reference } from "./errors.ts";
import { codeProblem, type Flow, type FlowEvent } from "./signInFlow.ts";
import { createWait } from "./wait.ts";

/** What a step's form says as a whole: its notice, or a refusal in general
 * terms with its request id. */
export function said(flow: Flow): { text: string | null; detail: string | undefined } {
  const f = flow.failure;
  return f === null
    ? { text: flow.notice, detail: undefined }
    : { text: describe(f), detail: reference(f) };
}

export function CodeStep(props: {
  /** The flow at its code step. */
  flow: Flow;
  dispatch: (event: FlowEvent) => void;
  /** What the step says above the field. */
  body: string;
  restart: string;
  onRestart: () => void;
  /** Signed in (the sign-in page follows the session instead). */
  onSignedIn?: () => void;
}) {
  const signal = componentSignal();
  const [code, setCode] = createSignal("");
  const [problem, setProblem] = createSignal<string | null>(null);
  // A recovery code has letters: a phone's digit pad would not take it.
  const [recovery, setRecovery] = createSignal(false);
  const [busy, setBusy] = createSignal(false);
  const wait = createWait();
  let field: HTMLInputElement | undefined;
  onCleanup(() => setCode(""));
  onMount(() => field?.focus());
  createEffect(
    on(
      () => props.flow,
      (flow) => {
        if (flow.wait !== null) wait.start(flow.wait);
        // A wrong code: the field is read again with its error.
        if (flow.fieldError !== null) field?.focus();
      },
    ),
  );

  const submit = async (e: SubmitEvent) => {
    e.preventDefault();
    const challenge = props.flow.challenge;
    const local = codeProblem(code());
    setProblem(local);
    if (local !== null || challenge === null || busy() || wait.seconds() > 0) return;
    props.dispatch({ type: "submit" });
    setBusy(true);
    try {
      await signInWithCode(challenge, code(), signal);
      props.onSignedIn?.();
    } catch (err) {
      if (signal.aborted) return;
      setCode("");
      props.dispatch({ type: "refused", error: asApiError(err) });
    } finally {
      setBusy(false);
    }
  };

  return (
    <form class="space-y-4" noValidate onSubmit={(e) => void submit(e)}>
      <p class="text-base text-muted">{props.body}</p>
      <Field
        label={msg("signIn.code")}
        autocomplete="one-time-code"
        inputMode={recovery() ? "text" : "numeric"}
        mono
        maxLength={64}
        hint={recovery() ? msg("signIn.recoveryHint") : msg("signIn.codeHint")}
        value={code()}
        ref={(el) => (field = el)}
        onInput={(v) => {
          setCode(v);
          if (problem() !== null) setProblem(codeProblem(v));
        }}
        error={problem() ?? props.flow.fieldError}
      />
      <Button
        class="h-7"
        variant="ghost"
        onClick={() => {
          setRecovery((r) => !r);
          field?.focus();
        }}
      >
        {recovery() ? msg("signIn.useApp") : msg("signIn.useRecovery")}
      </Button>
      <FormError message={said(props.flow).text} detail={said(props.flow).detail} />
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
        {busy() ? msg("signIn.codeSubmitting") : msg("signIn.codeSubmit")}
      </Button>
      <Button class="h-8 w-full" onClick={() => props.onRestart()}>
        {props.restart}
      </Button>
    </form>
  );
}
