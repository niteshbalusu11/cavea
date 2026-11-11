// Whole-page states before the console can show anything: the session
// loading, or the server not answering (with Retry, never a blank page).
import { Show } from "solid-js";
import { msg } from "../i18n/messages.ts";
import { loadSession, session } from "../session.ts";
import { Button } from "../ui/controls.tsx";
import { Card } from "../ui/form.tsx";
import { describe, reference } from "./errors.ts";

export function Loading() {
  return (
    <main class="flex h-full items-center justify-center bg-app">
      <p role="status" class="text-base text-muted">
        {msg("session.loading")}
      </p>
    </main>
  );
}

const error = () => {
  const s = session();
  return s.kind === "failed" ? s.error : null;
};

export function Failed() {
  return (
    <Card title={msg("session.failedTitle")}>
      <Show when={error()}>
        {(e) => (
          <>
            <p role="alert" class="text-base text-fg">
              {describe(e())}
            </p>
            <Show when={reference(e())}>{(r) => <p class="mt-1 text-sm text-muted">{r()}</p>}</Show>
          </>
        )}
      </Show>
      <Button variant="primary" class="mt-4 h-8 w-full" onClick={() => void loadSession()}>
        {msg("action.retry")}
      </Button>
    </Card>
  );
}
