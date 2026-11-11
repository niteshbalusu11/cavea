// Dialogs, toasts and menu styles. Kobalte traps focus in a dialog and
// returns it to where it was on close; Escape closes.
import { AlertDialog } from "@kobalte/core/alert-dialog";
import { Toast, toaster } from "@kobalte/core/toast";
import { Show, type JSX, type ParentProps } from "solid-js";
import { msg } from "../i18n/messages.ts";
import { Button, type Variant } from "./controls.tsx";
import { Icon } from "./Icon.tsx";

export const menuContent =
  "z-50 min-w-48 rounded-md bg-raised p-1 text-base text-fg shadow-menu outline-none";
export const menuItem =
  "flex h-7 cursor-default items-center gap-2 rounded-sm px-2 outline-none select-none data-disabled:opacity-45 data-highlighted:bg-hover";
export const menuSeparator = "my-1 h-px border-0 bg-line";
export const menuKeys = "ml-auto pl-4 text-sm text-faint";

/** A menu item's action that opens a dialog: run once the menu has closed
 * and put focus back on its trigger, so the dialog takes focus and returns
 * it there. */
export const afterMenu = (run: () => void) => () => void setTimeout(run, 0);

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
          <AlertDialog.Content class="w-full max-w-md rounded-lg bg-raised p-5 shadow-menu outline-none">
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
                  {props.onConfirm === undefined ? msg("action.close") : msg("action.cancel")}
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
