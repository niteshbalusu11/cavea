// The console's icons: a small set of 16 px stroked paths of our own, drawn
// in the current colour. Decorative: the control that holds one carries the
// accessible name.

const PATHS = {
  menu: "M2.5 4h11M2.5 8h11M2.5 12h11",
  undo: "M5.5 3 2.5 6l3 3M2.5 6h7a4 4 0 0 1 0 8H7",
  redo: "M10.5 3l3 3-3 3M13.5 6h-7a4 4 0 0 0 0 8H9",
  chevronDown: "m4 6 4 4 4-4",
  chevronRight: "m6 4 4 4-4 4",
  chevronLeft: "m10 4-4 4 4 4",
  search: "M7 2.5a4.5 4.5 0 1 0 0 9 4.5 4.5 0 0 0 0-9zM10.3 10.3 13.5 13.5",
  keyboard: "M1.5 4h13v8h-13zM4 6.5h.01M6.5 6.5h.01M9 6.5h.01M11.5 6.5h.01M5 9.5h6",
  select: "M3.5 2.5 12 8.2l-3.9.9 2.2 4.2-1.6.8-2.2-4.2L3.5 13z",
  hand: "M5.5 8V3.8a1 1 0 0 1 2 0V7.5m0-.5V2.8a1 1 0 0 1 2 0V7.5m0-3.2a1 1 0 0 1 2 0V9a5 5 0 0 1-5 5 4.6 4.6 0 0 1-3.9-2.2L1.9 9.6a1 1 0 0 1 1.6-1.2L5.5 10",
  rowBlock: "M2.5 3.5h2m2.5 0h2m2.5 0h2M2.5 8h2M7 8h2m2.5 0h2M2.5 12.5h2m2.5 0h2m2.5 0h2",
  arcBlock: "M2 11.5a6.5 6.5 0 0 1 12 0M4.5 13.5a4 4 0 0 1 7 0M3.6 5.6l.9.9M8 3.2v1.3m4.4 1.1-.9.9",
  row: "M2.5 9.5a1 1 0 1 0 0 .01M6 7.5a1 1 0 1 0 0 .01M10 7.5a1 1 0 1 0 0 .01M13.5 9.5a1 1 0 1 0 0 .01",
  rect: "M2.5 3.5h11v9h-11z",
  ellipse: "M8 3c3.3 0 6 2.2 6 5s-2.7 5-6 5-6-2.2-6-5 2.7-5 6-5z",
  polygon: "M8 2 14 6.5 11.5 13.5h-7L2 6.5z",
  line: "M3 13 13 3",
  text: "M3 3.5h10M8 3.5v9.5M6 13h4",
  section:
    "M2.5 2.5h2m2.5 0h2m2.5 0h2v2m0 2.5v2m0 2.5v2h-2m-2.5 0H7m-2.5 0h-2v-2m0-2.5V7m0-2.5V2.5",
  brush: "M10 2.5 13.5 6 7.5 12 4 8.5zM4 8.5 2.5 13.5 7.5 12",
  focal: "M8 1.5v3m0 7v3M1.5 8h3m7 0h3M8 5.5a2.5 2.5 0 1 0 0 5 2.5 2.5 0 0 0 0-5z",
  lock: "M4 7.5h8v6H4zM5.5 7.5V5a2.5 2.5 0 0 1 5 0v2.5",
  unlock: "M4 7.5h8v6H4zM5.5 7.5V5a2.5 2.5 0 0 1 4.9-.7",
  eye: "M1.5 8S4 3.5 8 3.5 14.5 8 14.5 8 12 12.5 8 12.5 1.5 8 1.5 8zM8 6a2 2 0 1 0 0 4 2 2 0 0 0 0-4z",
  eyeOff:
    "M2 2l12 12M6.6 3.7A6 6 0 0 1 8 3.5c4 0 6.5 4.5 6.5 4.5a12 12 0 0 1-1.7 2.2m-2.4 1.6a5.5 5.5 0 0 1-2.4.7C4 12.5 1.5 8 1.5 8a12 12 0 0 1 2.4-2.9",
  plus: "M8 3v10M3 8h10",
  trash: "M2.5 4.5h11M6 4.5V3h4v1.5M4 4.5l.7 9h6.6l.7-9",
  check: "m3 8.5 3 3 7-7",
  close: "m4 4 8 8m0-8-8 8",
  warning: "M8 2 14.5 13.5h-13zM8 6.5v3.5m0 1.8v.2",
  error: "M8 1.8a6.2 6.2 0 1 0 0 12.4A6.2 6.2 0 0 0 8 1.8zM8 4.8v3.8m0 2v.2",
  sun: "M8 5.2a2.8 2.8 0 1 0 0 5.6 2.8 2.8 0 0 0 0-5.6zM8 1v1.5m0 11V15M1 8h1.5m11 0H15M3 3l1 1m8 8 1 1m0-10-1 1m-8 8-1 1",
  moon: "M13.5 9.5A5.5 5.5 0 0 1 6.5 2.5a5.5 5.5 0 1 0 7 7z",
  monitor: "M2 3h12v8H2zM6 14h4M8 11v3",
  sidebar: "M2 3h12v10H2zM10 3v10",
  minus: "M3 8h10",
  more: "M3.5 8h.01M8 8h.01M12.5 8h.01",
  file: "M4 1.5h5.5L12.5 4.5v10h-8.5zM9.5 1.5v3h3",
  image: "M2 3h12v10H2zM2 11l3.5-3.5 3 3 2-2L14 12M10.5 5.5a1 1 0 1 0 0 .01",
  download: "M8 2v8m-3.5-3.5L8 10l3.5-3.5M2.5 13.5h11",
  chart: "M2.5 13.5h11M4.5 11V7m3.5 4V4m3.5 7V8.5",
  calendar: "M2.5 3.5h11v10h-11zM2.5 6.5h11M5.5 2v3m5-3v3",
  report: "M4 1.5h8v13H4zM6 5h4M6 8h4M6 11h2",
  code: "M5.5 4.5 2 8l3.5 3.5m5-7L14 8l-3.5 3.5",
  settings:
    "M8 5.5a2.5 2.5 0 1 0 0 5 2.5 2.5 0 0 0 0-5zM8 1.5v1.6m0 9.8v1.6M3.4 3.4l1.1 1.1m7 7 1.1 1.1M1.5 8h1.6m9.8 0h1.6M3.4 12.6l1.1-1.1m7-7 1.1-1.1",
  help: "M8 1.8a6.2 6.2 0 1 0 0 12.4A6.2 6.2 0 0 0 8 1.8zM6.2 6.2a1.9 1.9 0 0 1 3.7.6c0 1.3-1.9 1.6-1.9 2.8m0 1.8v.2",
  grip: "M6 3.5h.01M10 3.5h.01M6 8h.01M10 8h.01M6 12.5h.01M10 12.5h.01",
  sortUp: "M8 12.5v-9M4.5 7 8 3.5 11.5 7",
  sortDown: "M8 3.5v9M4.5 9 8 12.5 11.5 9",
  sortable: "m5 6 3-3 3 3M5 10l3 3 3-3",
  accessible: "M8.5 2.5h.01M8 4.8v4.2h3.2l1.6 3.3M8 6.8h3M5.6 6.6a3.6 3.6 0 1 0 5 5.1",
} as const;

export type IconName = keyof typeof PATHS;

export function Icon(props: { name: IconName; class?: string }) {
  return (
    <svg
      viewBox="0 0 16 16"
      width="16"
      height="16"
      fill="none"
      stroke="currentColor"
      stroke-width="1.4"
      stroke-linecap="round"
      stroke-linejoin="round"
      aria-hidden="true"
      class={props.class ?? "shrink-0"}
    >
      <path d={PATHS[props.name]} />
    </svg>
  );
}
