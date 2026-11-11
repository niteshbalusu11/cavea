// Prices for display (docs/05-web-sdk.md §3.1): the host's data, in minor
// units, shown in the legend and the tooltip and reported on objects. Cavea
// never charges anything; a price here is only text.
import type { Translate } from "./messages.ts";

export interface TicketTypePrice {
  key: string;
  /** Shown in the ticket-type choice; default the key. */
  label?: string;
  /** Minor units. */
  price: number;
}

/** One category's price, or its ticket types' (02 §1). */
export interface CategoryPricing {
  /** The category key. */
  category: string;
  /** Minor units. */
  price?: number;
  ticketTypes?: TicketTypePrice[];
}

export interface PriceContext {
  category: string;
  ticketType?: string;
  currency?: string;
  locale: string;
}

/** Text for a price in minor units. */
export type FormatPrice = (minor: number, context: PriceContext) => string;

const amount = (v: unknown): boolean => Number.isSafeInteger(v) && (v as number) >= 0;
const text = (v: unknown): boolean => typeof v === "string" && v !== "";
const isType = (t: unknown): boolean => {
  const o = t as Partial<TicketTypePrice> | null;
  return (
    typeof o === "object" &&
    o !== null &&
    text(o.key) &&
    (o.label === undefined || typeof o.label === "string") &&
    amount(o.price)
  );
};
const isEntry = (e: unknown): e is CategoryPricing => {
  const o = e as Partial<CategoryPricing> | null;
  return (
    typeof o === "object" &&
    o !== null &&
    text(o.category) &&
    (o.price === undefined || amount(o.price)) &&
    (o.ticketTypes === undefined ||
      (Array.isArray(o.ticketTypes) && o.ticketTypes.every(isType))) &&
    (o.price !== undefined || (o.ticketTypes?.length ?? 0) > 0)
  );
};

/** The well-formed entries of `pricing`; each other one is warned about and left out. */
export function checkPricing(
  pricing: unknown[],
  warn: (message: string) => void,
): CategoryPricing[] {
  return pricing.filter((entry, i) => {
    const ok = isEntry(entry);
    if (!ok) {
      warn(
        `pricing[${i}] is ignored: it needs a category, and a price or ticket types, in whole minor units`,
      );
    }
    return ok;
  });
}

/** The default formatter: the currency's own minor unit, or two decimals without a currency. */
function defaultFormat(locale: string, currency: string | undefined): (minor: number) => string {
  const format = new Intl.NumberFormat(
    locale,
    currency === undefined
      ? { minimumFractionDigits: 2, maximumFractionDigits: 2 }
      : { style: "currency", currency },
  );
  const digits = format.resolvedOptions().maximumFractionDigits ?? 2;
  return (minor) => format.format(minor / 10 ** digits);
}

export interface Prices {
  /** The category's ticket types; empty without. */
  types(category: string): TicketTypePrice[];
  /** A ticket type's price, else the category's. */
  price(category: string, ticketType?: string): number | undefined;
  /** A price as text, or the range over the category's ticket types; undefined without a price. */
  text(category: string, ticketType?: string): string | undefined;
}

export function prices(
  pricing: CategoryPricing[],
  format: FormatPrice | undefined,
  currency: string | undefined,
  locale: string,
  t: Translate,
  fail: (error: unknown) => void,
): Prices {
  const fallback = defaultFormat(locale, currency);
  const byKey = new Map(pricing.map((p) => [p.category, p]));
  const show = (minor: number, category: string, ticketType?: string): string => {
    if (format !== undefined) {
      try {
        const context: PriceContext = { category, locale };
        if (ticketType !== undefined) {
          context.ticketType = ticketType;
        }
        if (currency !== undefined) {
          context.currency = currency;
        }
        return String(format(minor, context));
      } catch (error) {
        fail(error);
      }
    }
    return fallback(minor);
  };
  const types = (category: string): TicketTypePrice[] => byKey.get(category)?.ticketTypes ?? [];
  const price = (category: string, ticketType?: string): number | undefined =>
    types(category).find((type) => type.key === ticketType)?.price ?? byKey.get(category)?.price;
  return {
    types,
    price,
    text(category, ticketType) {
      const all = types(category).map((type) => type.price);
      const one = price(category, ticketType);
      if (ticketType !== undefined || all.length === 0) {
        return one === undefined ? undefined : show(one, category, ticketType);
      }
      const [min, max] = [Math.min(...all), Math.max(...all)];
      return min === max
        ? show(min, category)
        : t("priceRange", { min: show(min, category), max: show(max, category) });
    },
  };
}
