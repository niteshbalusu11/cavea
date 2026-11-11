// The message catalogue (docs/05-web-sdk.md §3.4): every string the map can
// show, in English, keyed by stable ids. `messages` overrides any subset;
// `locale` picks plural forms and number formats. Placeholders are `{name}`;
// a message with a count is an object of `Intl.PluralRules` categories, at
// least `other`. No library: the browser has the rules.

/** A message: text, or plural forms chosen by the `count` value. */
export type Message = string | ({ other: string } & Partial<Record<Intl.LDMLPluralRule, string>>);

export const MESSAGES = {
  mapLabel: "Seat map",
  mapRole: "seat map",
  mapHelp: "Use the arrow keys to move between seats, and Enter to choose one.",
  place: "{index} of {count} in the row",
  seatSelected: "{seat} selected.",
  seatDeselected: "{seat} deselected.",
  selectedCount: {
    one: "{count} seat selected.",
    other: "{count} seats selected.",
  } as Message,
  zoomIn: "Zoom in",
  zoomOut: "Zoom out",
  zoomFit: "Show the whole map",
  legend: "Key",
  available: "Available",
  selected: "Selected",
  unavailable: "Unavailable",
  loading: "Loading the seat map…",
  loadFailed: "The seat map could not be loaded.",
  offline: "You are offline. The seat map will load when you are back online.",
  contextLost: "The seat map cannot be drawn right now.",
  retry: "Try again",
  soldOut: "Sold out",
  row: "Row {row}",
  seat: "Seat {seat}",
  priceRange: "{min} – {max}",
  accessible: "Wheelchair accessible",
  companion: "Companion seat",
  restrictedView: "Restricted view",
  aisle: "Aisle seat",
  ticketType: "Ticket type",
  taken: "{seat} was just taken by someone else.",
  notFree: "That seat is not available.",
  max: {
    one: "You can choose only one seat.",
    other: "You can choose up to {count} seats.",
  } as Message,
  holdLimit: "You cannot hold any more seats.",
  holdFailed: "That seat could not be held. Please try again.",
  holdLost: {
    one: "Your hold on one seat has ended.",
    other: "Your hold on {count} seats has ended.",
  } as Message,
  holdExpired: "Your hold expired and your seats were released.",
  reconnecting: "Live seat updates were interrupted. Reconnecting…",
  dismiss: "Dismiss",
} satisfies Record<string, Message>;

export type MessageId = keyof typeof MESSAGES;
export type Messages = Partial<Record<MessageId, Message>>;
export type Translate = (id: MessageId, values?: Record<string, string | number>) => string;

/** Whether `v` is a message: text, or plural forms with `other`. */
export const isMessage = (v: unknown): v is Message =>
  typeof v === "string" ||
  (typeof v === "object" &&
    v !== null &&
    typeof (v as Record<string, unknown>)["other"] === "string" &&
    Object.values(v).every((form) => typeof form === "string"));

/**
 * Messages for `locale`, `overrides` over English (unknown ids are ignored;
 * `validateOptions` warns about them). Numbers in `values` are formatted for
 * the locale; a `count` value picks the plural form.
 */
export function translator(locale: string, overrides: Messages | undefined): Translate {
  const plural = new Intl.PluralRules(locale);
  const number = new Intl.NumberFormat(locale);
  return (id, values = {}) => {
    const message: Message = overrides?.[id] ?? MESSAGES[id];
    const count = values["count"];
    const text =
      typeof message === "string"
        ? message
        : ((typeof count === "number" ? message[plural.select(count)] : undefined) ??
          message.other);
    return text.replace(/\{(\w+)\}/g, (whole, name: string) => {
      const value = values[name];
      return value === undefined ? whole : typeof value === "number" ? number.format(value) : value;
    });
  };
}

/** Languages written right to left, by primary subtag, and the scripts that are. */
const RTL = /^(ar|arc|ckb|dv|fa|he|iw|ps|sd|ug|ur|yi)(-|$)|-(arab|hebr|thaa|syrc|nkoo|adlm)(-|$)/i;

/** The text direction of `locale`'s script. */
export const localeDirection = (locale: string): "ltr" | "rtl" =>
  RTL.test(locale) ? "rtl" : "ltr";
