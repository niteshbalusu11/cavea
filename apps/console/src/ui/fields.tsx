// Form fields for dense panels: a label beside its control, "Mixed" where a
// selection differs, errors inline under the field (09 §8).
import { Checkbox } from "@kobalte/core/checkbox";
import { Select } from "@kobalte/core/select";
import { createSignal, createUniqueId, Show, type JSX } from "solid-js";
import { formatNumber, msg } from "../i18n/messages.ts";
import { Icon } from "./Icon.tsx";

const inputClass =
  "h-6 w-full min-w-0 rounded-sm border border-control-line bg-control px-1.5 text-base text-fg placeholder:text-faint aria-invalid:border-danger disabled:opacity-60";

function Row(props: {
  label: JSX.Element;
  /** The control's id: the label is a `<label>` for it, else plain text the
   * control names itself by. */
  for?: string;
  error?: string | null;
  errorId?: string;
  children: JSX.Element;
}) {
  return (
    <div class="grid grid-cols-[88px_minmax(0,1fr)] items-center gap-x-2 gap-y-0.5 px-3 py-0.5">
      <Show
        when={props.for}
        fallback={<span class="truncate text-sm text-muted">{props.label}</span>}
      >
        <label for={props.for} class="truncate text-sm text-muted">
          {props.label}
        </label>
      </Show>
      {props.children}
      <Show when={props.error}>
        <p id={props.errorId} class="col-start-2 text-xs text-danger" role="alert">
          {props.error}
        </p>
      </Show>
    </div>
  );
}

/** A text field that commits on Enter or blur; Escape puts the value back. */
export function TextField(props: {
  label: string;
  value: string | null;
  mixed?: boolean;
  placeholder?: string;
  /** Returns an error message, or `null` when the value was taken. */
  onCommit: (value: string) => string | null;
}) {
  const id = createUniqueId();
  const [error, setError] = createSignal<string | null>(null);
  const shown = () => (props.mixed === true ? "" : (props.value ?? ""));
  const commit = (input: HTMLInputElement) => {
    if (input.value === shown()) return setError(null);
    const problem = props.onCommit(input.value);
    setError(problem);
    if (problem === null) input.value = shown();
  };
  return (
    <Row label={props.label} for={id} error={error()} errorId={`${id}-error`}>
      <input
        id={id}
        class={inputClass}
        value={shown()}
        placeholder={props.mixed === true ? msg("inspector.mixed") : props.placeholder}
        aria-invalid={error() !== null}
        aria-describedby={error() !== null ? `${id}-error` : undefined}
        onKeyDown={(e) => {
          if (e.key === "Enter") commit(e.currentTarget);
          if (e.key === "Escape") {
            e.currentTarget.value = shown();
            setError(null);
            e.currentTarget.blur();
          }
        }}
        onBlur={(e) => commit(e.currentTarget)}
      />
    </Row>
  );
}

export interface Scrub {
  begin(): boolean;
  set(value: number): void;
  end(): void;
  cancel(): void;
}

/**
 * A number field. Typed values commit on Enter or blur, checked against
 * `min` and `max` first; the arrow keys step by `step` (10× with Shift).
 * Dragging the label sideways scrubs: `begin`, `set` on every move, `end`
 * on release, `cancel` on Escape (07 §9).
 */
export function NumberField(props: {
  label: string;
  value: number | null;
  min: number;
  max: number;
  unit: string;
  step?: number;
  readOnly?: boolean;
  onCommit: (value: number) => string | null;
  scrub?: Scrub;
}) {
  const id = createUniqueId();
  const [error, setError] = createSignal<string | null>(null);
  const [scrubbing, setScrubbing] = createSignal(false);
  const step = () => props.step ?? 1;
  const text = () => (props.value === null ? "" : formatNumber(props.value));
  const clamp = (v: number) => Math.min(props.max, Math.max(props.min, v));
  let input = document.createElement("input");

  const take = (value: number): void => {
    if (!Number.isFinite(value)) return void setError(msg("inspector.notNumber"));
    if (value < props.min || value > props.max) {
      return void setError(msg("inspector.range", { min: props.min, max: props.max }));
    }
    setError(props.onCommit(value));
    input.value = text();
  };
  const commit = () => {
    const raw = input.value.replaceAll(",", "").trim();
    if (raw === "" || raw === text().replaceAll(",", "")) {
      input.value = text();
      return setError(null);
    }
    take(Number(raw));
  };

  const startScrub = (e: PointerEvent) => {
    const scrub = props.scrub;
    if (scrub === undefined || props.readOnly === true || e.button !== 0 || props.value === null)
      return;
    // A press on the label would focus the field; a scrub keeps focus where it is.
    e.preventDefault();
    const from = props.value;
    const x0 = e.clientX;
    const target = e.currentTarget as HTMLElement;
    let open = false;
    const move = (m: PointerEvent) => {
      const dx = m.clientX - x0;
      if (!open) {
        if (Math.abs(dx) < 3 || !scrub.begin()) return;
        open = true;
        setScrubbing(true);
      }
      const units = Math.round(dx / 2) * step() * (m.shiftKey ? 10 : 1);
      scrub.set(clamp(from + units));
    };
    const stop = (cancelled: boolean) => {
      target.removeEventListener("pointermove", move);
      target.removeEventListener("pointerup", up);
      target.removeEventListener("pointercancel", lost);
      window.removeEventListener("keydown", key, true);
      if (open) (cancelled ? scrub.cancel : scrub.end)();
      setScrubbing(false);
    };
    const up = () => stop(false);
    const lost = () => stop(true);
    const key = (k: KeyboardEvent) => {
      if (k.key !== "Escape") return;
      k.preventDefault();
      k.stopPropagation();
      stop(true);
    };
    target.setPointerCapture(e.pointerId);
    target.addEventListener("pointermove", move);
    target.addEventListener("pointerup", up);
    target.addEventListener("pointercancel", lost);
    window.addEventListener("keydown", key, true);
  };

  const scrubs = () => props.scrub !== undefined && props.readOnly !== true;
  const label = (
    <span
      id={`${id}-label`}
      class={`select-none ${props.scrub !== undefined && props.readOnly !== true ? "cursor-ew-resize" : ""} ${scrubbing() ? "text-accent" : ""}`}
      onPointerDown={startScrub}
    >
      {props.label}
    </span>
  );

  return (
    // A scrubbing label is not a <label>: a press on it scrubs, not focuses.
    <Row label={label} {...(scrubs() ? {} : { for: id })} error={error()} errorId={`${id}-error`}>
      <div class="relative">
        <input
          ref={(e) => (input = e)}
          id={id}
          aria-labelledby={scrubs() ? `${id}-label` : undefined}
          class={`${inputClass} pr-7 tabular-nums`}
          inputmode="decimal"
          value={text()}
          readOnly={props.readOnly}
          placeholder={props.value === null ? msg("inspector.mixed") : undefined}
          aria-invalid={error() !== null}
          aria-describedby={error() !== null ? `${id}-error` : undefined}
          onKeyDown={(e) => {
            if (e.key === "Enter") commit();
            else if (e.key === "Escape") {
              input.value = text();
              setError(null);
              input.blur();
            } else if (
              (e.key === "ArrowUp" || e.key === "ArrowDown") &&
              props.value !== null &&
              props.readOnly !== true
            ) {
              e.preventDefault();
              const by = step() * (e.shiftKey ? 10 : 1) * (e.key === "ArrowUp" ? 1 : -1);
              take(clamp(props.value + by));
            }
          }}
          onBlur={commit}
        />
        <span class="pointer-events-none absolute inset-y-0 right-1.5 flex items-center text-xs text-faint">
          {props.unit}
        </span>
      </div>
    </Row>
  );
}

export interface Option {
  value: string;
  label: string;
}

/** A choice among a few values; "Mixed" when the selection differs. */
export function SelectField(props: {
  label: string;
  options: Option[];
  value: string | null;
  onChange: (value: string) => void;
}) {
  const id = createUniqueId();
  const current = () => props.options.find((o) => o.value === props.value) ?? null;
  return (
    <Row label={<span id={`${id}-label`}>{props.label}</span>}>
      <Select<Option>
        options={props.options}
        optionValue="value"
        optionTextValue="label"
        value={current()}
        onChange={(o) => {
          if (o !== null && o.value !== props.value) props.onChange(o.value);
        }}
        placeholder={msg("inspector.mixed")}
        gutter={4}
        itemComponent={(p) => (
          <Select.Item
            item={p.item}
            class="flex h-7 cursor-default items-center justify-between gap-3 rounded-sm px-2 text-base text-fg outline-none data-highlighted:bg-hover"
          >
            <Select.ItemLabel>{p.item.rawValue.label}</Select.ItemLabel>
            <Select.ItemIndicator>
              <Icon name="check" />
            </Select.ItemIndicator>
          </Select.Item>
        )}
      >
        <Select.Trigger
          aria-labelledby={`${id}-label`}
          class={`${inputClass} flex items-center justify-between text-left`}
        >
          <Select.Value<Option> class="truncate data-placeholder-shown:text-faint">
            {(state) => state.selectedOption().label}
          </Select.Value>
          <Select.Icon class="text-faint">
            <Icon name="chevronDown" />
          </Select.Icon>
        </Select.Trigger>
        <Select.Portal>
          <Select.Content class="z-50 min-w-32 rounded-md bg-raised p-1 shadow-menu">
            <Select.Listbox class="max-h-72 overflow-y-auto outline-none" />
          </Select.Content>
        </Select.Portal>
      </Select>
    </Row>
  );
}

/** A checkbox; indeterminate where the selection differs. */
export function CheckField(props: {
  label: string;
  checked: boolean | null;
  onChange: (on: boolean) => void;
}) {
  return (
    <div class="px-3 py-0.5 pl-[108px]">
      <Checkbox
        class="inline-flex items-center gap-2"
        checked={props.checked === true}
        indeterminate={props.checked === null}
        onChange={(on) => props.onChange(on)}
      >
        <Checkbox.Input class="peer sr-only" />
        <Checkbox.Control class="flex size-3.5 items-center justify-center rounded-sm border border-control-line bg-control text-on-accent peer-focus-visible:outline-2 peer-focus-visible:outline-focus data-checked:border-accent-fill data-checked:bg-accent-fill data-indeterminate:border-accent-fill data-indeterminate:bg-accent-fill">
          <Checkbox.Indicator>
            <Icon name={props.checked === null ? "minus" : "check"} class="size-3" />
          </Checkbox.Indicator>
        </Checkbox.Control>
        <Checkbox.Label class="text-sm text-fg">{props.label}</Checkbox.Label>
      </Checkbox>
    </div>
  );
}
