// A separator that resizes the regions beside it: by pointer, or by the
// arrow keys when focused (Home and End for the limits).

export function Splitter(props: {
  /** `vertical`: a vertical bar between regions side by side. */
  orientation: "vertical" | "horizontal";
  label: string;
  value: number;
  min: number;
  max: number;
  step: number;
  /** The value after the pointer moved `delta` px from where the drag
   * started at `start`. */
  fromDrag: (start: number, delta: number) => number;
  onChange: (value: number) => void;
}) {
  const clamp = (v: number) => Math.min(props.max, Math.max(props.min, v));
  const vertical = () => props.orientation === "vertical";
  const down = (e: PointerEvent) => {
    if (e.button !== 0) return;
    const el = e.currentTarget as HTMLElement;
    const start = props.value;
    const p0 = vertical() ? e.clientX : e.clientY;
    el.setPointerCapture(e.pointerId);
    const move = (m: PointerEvent) =>
      props.onChange(clamp(props.fromDrag(start, (vertical() ? m.clientX : m.clientY) - p0)));
    const up = () => {
      el.removeEventListener("pointermove", move);
      el.removeEventListener("pointerup", up);
      el.removeEventListener("pointercancel", up);
    };
    el.addEventListener("pointermove", move);
    el.addEventListener("pointerup", up);
    el.addEventListener("pointercancel", up);
  };
  const key = (e: KeyboardEvent) => {
    const less = vertical() ? "ArrowLeft" : "ArrowUp";
    const more = vertical() ? "ArrowRight" : "ArrowDown";
    const by = props.step * (e.shiftKey ? 5 : 1);
    const next =
      e.key === less
        ? props.value - by
        : e.key === more
          ? props.value + by
          : e.key === "Home"
            ? props.min
            : e.key === "End"
              ? props.max
              : null;
    if (next === null) return;
    e.preventDefault();
    props.onChange(clamp(next));
  };
  return (
    <div
      role="separator"
      tabIndex={0}
      aria-label={props.label}
      aria-orientation={props.orientation}
      aria-valuenow={Math.round(props.value * 100) / 100}
      aria-valuemin={props.min}
      aria-valuemax={props.max}
      class={`group relative z-10 shrink-0 ${vertical() ? "-mx-1 w-2 cursor-col-resize" : "-my-1 h-2 cursor-row-resize"}`}
      onPointerDown={down}
      onKeyDown={key}
    >
      <div
        class={`absolute bg-transparent group-hover:bg-accent-fill group-focus-visible:bg-accent-fill ${vertical() ? "inset-y-0 left-[3px] w-0.5" : "inset-x-0 top-[3px] h-0.5"}`}
      />
    </div>
  );
}
