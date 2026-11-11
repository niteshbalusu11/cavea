// Buttons and tooltips. Every control has a visible focus ring (the base
// `:focus-visible` style) and an accessible name.
import { Button as KButton } from "@kobalte/core/button";
import { Tooltip } from "@kobalte/core/tooltip";
import { Show, type ParentProps } from "solid-js";
import { Icon, type IconName } from "./Icon.tsx";

export type Variant = "primary" | "secondary" | "ghost" | "danger";

const VARIANTS: Record<Variant, string> = {
  primary: "bg-accent-fill text-on-accent hover:brightness-110 active:brightness-95",
  secondary: "bg-control text-fg border border-line-strong hover:bg-hover active:bg-active",
  ghost: "text-fg hover:bg-hover active:bg-active",
  danger: "bg-danger-soft text-danger border border-danger hover:brightness-95",
};

export const buttonClass = (variant: Variant = "secondary", extra = "") =>
  `inline-flex h-7 items-center justify-center gap-1.5 rounded-md px-2.5 text-base font-medium whitespace-nowrap select-none disabled:cursor-not-allowed disabled:opacity-45 aria-disabled:cursor-not-allowed aria-disabled:opacity-45 ${VARIANTS[variant]} ${extra}`;

interface ButtonProps extends ParentProps {
  variant?: Variant;
  icon?: IconName;
  onClick?: () => void;
  disabled?: boolean;
  class?: string;
  type?: "button" | "submit";
}

export function Button(props: ButtonProps) {
  return (
    <KButton
      type={props.type ?? "button"}
      class={buttonClass(props.variant, props.class)}
      disabled={props.disabled}
      onClick={() => props.onClick?.()}
    >
      <Show when={props.icon}>{(name) => <Icon name={name()} />}</Show>
      {props.children}
    </KButton>
  );
}

/** A hint on hover and focus, with the shortcut in a quieter colour. */
type Placement = "top" | "bottom" | "right" | "left";

export function Tip(
  props: ParentProps<{
    label: string;
    keys?: string | null | undefined;
    placement?: Placement | undefined;
  }>,
) {
  return (
    <Tooltip placement={props.placement ?? "bottom"} openDelay={500} closeDelay={0} gutter={6}>
      {props.children}
      <Tooltip.Portal>
        <Tooltip.Content class="z-50 max-w-72 rounded-md bg-raised px-2 py-1 text-sm text-fg shadow-menu">
          {props.label}
          <Show when={props.keys}>
            <span class="ml-2 text-faint">{props.keys}</span>
          </Show>
        </Tooltip.Content>
      </Tooltip.Portal>
    </Tooltip>
  );
}

interface IconButtonProps {
  icon: IconName;
  label: string;
  keys?: string | null;
  onClick?: (e: MouseEvent) => void;
  /** Disabled, with the reason (or "" when the label says it): still
   * focusable, so the tooltip says why. */
  reason?: string | null;
  pressed?: boolean;
  placement?: Placement;
  /** Replaces the default size, `size-7`. */
  class?: string;
  tabIndex?: number;
}

export function IconButton(props: IconButtonProps) {
  const blocked = () => props.reason !== undefined && props.reason !== null;
  return (
    <Tip
      label={blocked() && props.reason !== "" ? `${props.label}: ${props.reason}` : props.label}
      keys={props.keys}
      placement={props.placement}
    >
      <Tooltip.Trigger
        as="button"
        type="button"
        aria-label={props.label}
        aria-pressed={props.pressed}
        aria-disabled={blocked() ? true : undefined}
        tabIndex={props.tabIndex}
        class={`inline-flex shrink-0 items-center justify-center rounded-md text-muted hover:bg-hover hover:text-fg aria-disabled:opacity-45 aria-pressed:bg-accent-soft aria-pressed:text-accent ${props.class ?? "size-7"}`}
        onClick={(e: MouseEvent) => {
          if (!blocked()) props.onClick?.(e);
        }}
      >
        <Icon name={props.icon} />
      </Tooltip.Trigger>
    </Tip>
  );
}
