// Dialogs, toasts and menu styles. Kobalte traps focus in a dialog and
// returns it to where it was on close (or, when that is gone, `focusReturn`
// finds where); Escape closes.
import { AlertDialog } from "@kobalte/core/alert-dialog";
// The dialog's parts by name: Kobalte builds `Dialog` and `AlertDialog` by
// assigning their parts onto one shared root function, so with both
// imported `Dialog.Content` would be the alert dialog's (role
// `alertdialog`) for every dialog.
import {
  Content as DialogContent,
  Overlay as DialogOverlay,
  Portal as DialogPortal,
  Root as DialogRoot,
  Title as DialogTitle,
} from "@kobalte/core/dialog";
import { Toast, toaster } from "@kobalte/core/toast";
import { createComputed, on, Show, type JSX, type ParentProps } from "solid-js";
import { documentPage, focusOn, opener, replacement, type Opener } from "./focus.ts";
import { msg } from "../i18n/messages.ts";
import { Button, type Variant } from "./controls.tsx";
import { Icon } from "./Icon.tsx";

export const menuContent =
  "z-50 min-w-48 rounded-md bg-raised p-1 text-base text-fg shadow-menu outline-none";
export const menuItem =
  "flex h-7 cursor-default items-center gap-2 rounded-sm px-2 outline-none select-none data-disabled:opacity-45 data-highlighted:bg-active";
export const menuSeparator = "my-1 h-px border-0 bg-line";
export const menuKeys = "ml-auto pl-4 text-sm text-faint";

/** The trigger of the menu whose item opened the next dialog: that dialog's
 * opener, taken by `focusReturn` (while the menu closes, the page around
 * it is still inert, so the trigger cannot simply be focused). */
let menuOpener: HTMLElement | null = null;

/** A menu item's action that opens a dialog: run once the menu has closed;
 * the dialog returns focus to the menu's trigger. */
export const afterMenu = (run: () => void) => () => {
  const item = document.activeElement;
  const menu = item instanceof HTMLElement ? item.closest("[role=menu]") : null;
  const by = menu?.getAttribute("aria-labelledby");
  const trigger = by === null || by === undefined ? null : document.getElementById(by);
  setTimeout(() => {
    menuOpener = trigger;
    run();
    menuOpener = null;
  }, 0);
};

/** A dialog's focus on open and close: back to the element that opened it,
 * or, when that is gone (its row drawn again or deleted), to what took its
 * place or the page's heading, never to the page's body. The opener is
 * read when `open` turns true, before the dialog mounts and takes focus. */
function focusReturn(open: () => boolean, initial?: () => HTMLElement | undefined) {
  let from: Opener<HTMLElement> | null = null;
  createComputed(
    on(open, (now) => {
      if (now) from = opener(menuOpener);
    }),
  );
  return {
    onOpenAutoFocus: (e: Event) => {
      const el = initial?.();
      if (el === undefined) return;
      e.preventDefault();
      el.focus();
    },
    onCloseAutoFocus: (e: Event) => {
      const back = from;
      from = null;
      e.preventDefault();
      const target = replacement(back, documentPage) ?? back?.el ?? null;
      if (target !== null) focusOn(target);
    },
  };
}

/**
 * A modal dialog that asks before something: a title, what it will do,
 * optional controls, and the actions. `onConfirm` returning false keeps it
 * open. Without `onConfirm` it only informs.
 */
export function Confirm(
  props: ParentProps<{
    open: boolean;
    title: string;
    description: string;
    confirm?: string;
    /** The other button's words: "Cancel" by default. */
    cancel?: string;
    variant?: Variant;
    onConfirm?: () => boolean | void;
    onClose: () => void;
    /** No way out but the action: the dialog cannot be dismissed. */
    blocking?: boolean;
  }>,
) {
  return (
    <AlertDialog
      open={props.open}
      onOpenChange={(open) => {
        if (!open && props.blocking !== true) props.onClose();
      }}
    >
      <AlertDialog.Portal>
        <AlertDialog.Overlay class="fixed inset-0 z-40 bg-scrim" />
        <div class="fixed inset-0 z-50 flex items-center justify-center p-4">
          <AlertDialog.Content
            class="w-full max-w-md rounded-lg bg-raised p-5 shadow-menu outline-none"
            {...focusReturn(() => props.open)}
          >
            <AlertDialog.Title class="text-lg font-semibold text-fg">
              {props.title}
            </AlertDialog.Title>
            <AlertDialog.Description class="mt-2 text-base text-muted">
              {props.description}
            </AlertDialog.Description>
            {props.children}
            <div class="mt-5 flex justify-end gap-2">
              <Show when={props.blocking !== true}>
                <Button onClick={() => props.onClose()}>
                  {props.onConfirm === undefined
                    ? msg("action.close")
                    : (props.cancel ?? msg("action.cancel"))}
                </Button>
              </Show>
              <Show when={props.onConfirm}>
                {(run) => (
                  <Button
                    variant={props.variant ?? "primary"}
                    onClick={() => {
                      if (run()() !== false) props.onClose();
                    }}
                  >
                    {props.confirm ?? msg("action.ok")}
                  </Button>
                )}
              </Show>
            </div>
          </AlertDialog.Content>
        </div>
      </AlertDialog.Portal>
    </AlertDialog>
  );
}

/**
 * A modal dialog for a tool of its own (the palette, the shortcut help, a
 * form): focus is trapped inside and returns where it was on close;
 * Escape and a press outside close it. `top` places it near the top of the
 * window, as a palette sits, rather than centred.
 */
export function Dialog(
  props: ParentProps<{
    open: boolean;
    title: string;
    /** Shown as the title; otherwise the title is for assistive technology only. */
    heading?: boolean;
    onClose: () => void;
    class?: string;
    top?: boolean;
    /** Focus this on open instead of the first focusable element. */
    initial?: () => HTMLElement | undefined;
  }>,
) {
  return (
    <DialogRoot open={props.open} onOpenChange={(open) => !open && props.onClose()}>
      <DialogPortal>
        <DialogOverlay class="fixed inset-0 z-40 bg-scrim" />
        <div
          class="fixed inset-0 z-50 flex justify-center p-4"
          classList={{
            "items-start pt-[12vh]": props.top === true,
            "items-center": props.top !== true,
          }}
        >
          <DialogContent
            class={`flex max-h-full w-full flex-col rounded-lg bg-raised shadow-menu outline-none ${props.class ?? "max-w-md"}`}
            {...focusReturn(
              () => props.open,
              () => props.initial?.(),
            )}
          >
            <DialogTitle
              class={
                props.heading === true
                  ? "shrink-0 px-5 pt-4 pb-1 text-lg font-semibold text-fg"
                  : "sr-only"
              }
            >
              {props.title}
            </DialogTitle>
            {props.children}
          </DialogContent>
        </div>
      </DialogPortal>
    </DialogRoot>
  );
}

export type Tone = "info" | "error";

/** A toast for a background result or a refusal: announced politely,
 * dismissed after a few seconds or by its button. */
export function notify(text: string, tone: Tone = "info", detail?: string): void {
  toaster.show((p) => (
    <Toast
      toastId={p.toastId}
      duration={tone === "error" ? 6_000 : 3_000}
      class="pointer-events-auto flex w-80 items-start gap-2 rounded-md bg-raised p-3 shadow-menu"
    >
      <Show when={tone === "error"}>
        <span class="mt-px text-danger">
          <Icon name="error" />
        </span>
      </Show>
      <div class="min-w-0 flex-1">
        <Toast.Title class="text-base text-fg">{text}</Toast.Title>
        <Show when={detail}>
          <Toast.Description class="mt-0.5 text-sm break-words text-muted">
            {detail}
          </Toast.Description>
        </Show>
      </div>
      <Toast.CloseButton
        class="rounded-sm text-faint hover:text-fg"
        aria-label={msg("action.close")}
      >
        <Icon name="close" />
      </Toast.CloseButton>
    </Toast>
  ));
}

export function ToastRegion(): JSX.Element {
  return (
    <Toast.Region limit={4} aria-label={msg("app.name")}>
      <Toast.List class="pointer-events-none fixed right-3 bottom-9 z-50 flex flex-col gap-2 outline-none" />
    </Toast.Region>
  );
}
