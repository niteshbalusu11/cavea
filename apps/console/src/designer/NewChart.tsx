// The new-chart screen (07 §16): draw from scratch, start from a template
// (theatre, arena bowl, banquet), or place a reference image, and open a
// chart file. It is the designer's empty state and a dialog from the
// designer's menu.
import { For, type JSX } from "solid-js";
import { formatNumber, msg, type MessageId } from "../i18n/messages.ts";
import { Tip } from "../ui/controls.tsx";
import { Icon, type IconName } from "../ui/Icon.tsx";
import { Dialog } from "../ui/overlays.tsx";
import { Tooltip } from "@kobalte/core/tooltip";
import { TEMPLATES, type Template } from "./templates.ts";

const ICON: Record<Template["id"], IconName> = {
  theatre: "arcBlock",
  arena: "ellipse",
  banquet: "rowBlock",
};

export interface Choices {
  empty: () => void;
  template: (t: Template) => void;
  open: () => void;
}

function Card(props: {
  icon: IconName;
  title: string;
  body: string;
  onClick?: () => void;
  disabled?: string;
}) {
  const inner = (
    <>
      <span class="flex size-8 shrink-0 items-center justify-center rounded-md bg-hover text-muted group-enabled:group-hover:text-accent">
        <Icon name={props.icon} />
      </span>
      <span class="min-w-0">
        <span class="block text-base font-medium text-fg">{props.title}</span>
        <span class="block text-sm text-muted">{props.body}</span>
      </span>
    </>
  );
  const cls =
    "group flex w-full items-start gap-3 rounded-lg border border-line bg-raised p-3 text-left hover:border-line-strong hover:bg-hover aria-disabled:cursor-not-allowed aria-disabled:opacity-60 aria-disabled:hover:border-line aria-disabled:hover:bg-raised";
  if (props.disabled !== undefined) {
    return (
      <Tip label={props.disabled}>
        <Tooltip.Trigger as="button" type="button" aria-disabled="true" class={cls}>
          {inner}
        </Tooltip.Trigger>
      </Tip>
    );
  }
  return (
    <button type="button" class={cls} onClick={() => props.onClick?.()}>
      {inner}
    </button>
  );
}

/** The three starting points and the file picker. */
export function NewChart(props: { choices: Choices }): JSX.Element {
  return (
    <div class="w-full max-w-2xl">
      <div class="grid grid-cols-2 gap-2">
        <Card
          icon="plus"
          title={msg("new.empty")}
          body={msg("new.emptyBody")}
          onClick={props.choices.empty}
        />
        <Card
          icon="file"
          title={msg("new.image")}
          body={msg("new.imageBody")}
          disabled={msg("new.imagePending")}
        />
      </div>
      <h2 class="mt-5 mb-2 text-xs font-semibold tracking-wide text-faint uppercase">
        {msg("new.templates")}
      </h2>
      <div class="grid grid-cols-3 gap-2">
        <For each={TEMPLATES}>
          {(t) => (
            <Card
              icon={ICON[t.id]}
              title={msg(`template.${t.id}` as MessageId)}
              body={msg(`template.${t.id}Body` as MessageId, { seats: formatNumber(t.seats) })}
              onClick={() => props.choices.template(t)}
            />
          )}
        </For>
      </div>
      <p class="mt-5 text-sm text-muted">
        {msg("new.or")}{" "}
        <button
          type="button"
          class="rounded-sm text-accent underline-offset-2 hover:underline"
          onClick={() => props.choices.open()}
        >
          {msg("menu.open")}
        </button>
      </p>
    </div>
  );
}

export function NewChartDialog(props: { open: boolean; choices: Choices; onClose: () => void }) {
  const close = (run: () => void) => () => {
    props.onClose();
    setTimeout(run, 0);
  };
  return (
    <Dialog
      open={props.open}
      title={msg("new.title")}
      heading
      onClose={props.onClose}
      class="max-w-2xl"
    >
      <div class="px-5 pt-1 pb-5">
        <NewChart
          choices={{
            empty: close(props.choices.empty),
            template: (t) => close(() => props.choices.template(t))(),
            open: close(props.choices.open),
          }}
        />
      </div>
    </Dialog>
  );
}
