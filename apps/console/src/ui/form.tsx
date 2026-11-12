// Form fields for the console's pages (09 §8): a label above its control,
// a hint, and an error under the field tied to it by `aria-describedby`.
// A form's own failure (a refusal that names no field) is an alert above
// its actions.
import { createUniqueId, Show, type JSX } from "solid-js";
import { Icon } from "./Icon.tsx";

export const fieldInputClass =
  "h-8 w-full min-w-0 rounded-md border border-control-line bg-control px-2.5 text-base text-fg placeholder:text-faint aria-invalid:border-danger disabled:opacity-60";

export function Field(props: {
  label: string;
  value: string;
  onInput: (value: string) => void;
  onBlur?: () => void;
  type?: "text" | "email" | "password";
  autocomplete?: string;
  hint?: string;
  error?: string | null;
  disabled?: boolean;
  mono?: boolean;
  maxLength?: number;
  ref?: (el: HTMLInputElement) => void;
}) {
  const id = createUniqueId();
  const described = () =>
    [props.hint === undefined ? null : `${id}-hint`, props.error ? `${id}-error` : null]
      .filter((x) => x !== null)
      .join(" ") || undefined;
  return (
    <div class="space-y-1">
      <label for={id} class="block text-sm font-medium text-fg">
        {props.label}
      </label>
      <input
        id={id}
        ref={(el) => props.ref?.(el)}
        type={props.type ?? "text"}
        class={`${fieldInputClass} ${props.mono === true ? "font-mono" : ""}`}
        value={props.value}
        autocomplete={props.autocomplete}
        spellcheck={false}
        maxLength={props.maxLength}
        disabled={props.disabled}
        aria-invalid={props.error ? true : undefined}
        aria-describedby={described()}
        onInput={(e) => props.onInput(e.currentTarget.value)}
        onBlur={() => props.onBlur?.()}
      />
      <Show when={props.hint}>
        <p id={`${id}-hint`} class="text-sm text-muted">
          {props.hint}
        </p>
      </Show>
      <Show when={props.error}>
        <p id={`${id}-error`} class="text-sm text-danger">
          {props.error}
        </p>
      </Show>
    </div>
  );
}

/** A failure of the whole form, announced as it appears. */
export function FormError(props: { message: string | null; detail?: string | undefined }) {
  return (
    <Show when={props.message}>
      <div
        role="alert"
        class="rounded-md border border-danger bg-danger-soft px-3 py-2 text-base text-danger"
      >
        {props.message}
        <Show when={props.detail}>
          <span class="mt-0.5 block text-sm">{props.detail}</span>
        </Show>
      </div>
    </Show>
  );
}

/** Characters as the server counts them (Unicode scalar values). */
export const length = (s: string): number => [...s].length;

export const PASSWORD_MIN = 15;
export const PASSWORD_MAX = 128;

/** A page outside the frame: sign-in, setup, a start that failed. */
export function Card(props: { title: string; children: JSX.Element }) {
  return (
    <main class="flex min-h-full items-center justify-center bg-app p-4">
      <div class="w-full max-w-sm rounded-lg bg-surface p-6 shadow-menu">
        <h1 class="mb-4 flex items-center gap-2 text-lg font-semibold text-fg">
          <span class="flex size-6 items-center justify-center rounded-md bg-accent-fill text-on-accent">
            <Icon name="section" class="size-3.5" />
          </span>
          {props.title}
        </h1>
        {props.children}
      </div>
    </main>
  );
}
