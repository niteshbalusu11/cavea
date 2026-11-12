// The licence in the console's words (12 §7.3, 09 §7): which banner a
// person sees, why a file is invalid, why a chart or event control is
// closed. Pure functions over `me.licence`, so the rules are tested once.
import type { ConsoleLicence, OperationId } from "../api/generated.ts";
import { CLOSED_WHILE_EXPIRED, refused, type Permission } from "../api/permissions.ts";
import { msg, msgOf } from "../i18n/messages.ts";
import { workspace } from "../session.ts";

export type Banner = "grace" | "expired" | "development" | null;

/**
 * The banner for a person (12 §7.3's console column): none while valid; in
 * grace, for owners and admins (who manage the server, or may open
 * Settings); expired, for everyone whose work it closes (a role that writes
 * charts or events) and whoever can renew it; missing or invalid, the
 * development notice for everyone.
 */
export function bannerFor(licence: ConsoleLicence, permissions: readonly Permission[]): Banner {
  switch (licence.state) {
    case "valid":
      return null;
    case "grace":
      return licence.manage || permissions.includes("settings") ? "grace" : null;
    case "expired":
      return licence.manage || permissions.some((p) => CLOSED_WHILE_EXPIRED.includes(p))
        ? "expired"
        : null;
    case "missing":
    case "invalid":
      return "development";
  }
}

/** What to do about an invalid licence's `reason`, in words. */
export const reasonText = (reason: string): string =>
  msgOf("licence.reason", reason) ?? msg("licence.reasonOther", { reason });

/**
 * Why the person may not call `op` now, in words, or `null` when they may:
 * `role()` for their role, the licence's words while it is expired.
 */
export function cannot(op: OperationId, role: () => string): string | null {
  switch (refused(workspace(), op)) {
    case "role":
      return role();
    case "licence":
      return msg("licence.readOnly");
    default:
      return null;
  }
}

/** A licence date (`YYYY-MM-DD`, a UTC day) in the person's locale. */
export function licenceDate(day: string, locale?: string): string {
  const ms = Date.parse(`${day}T00:00:00Z`);
  if (Number.isNaN(ms)) return day;
  return new Intl.DateTimeFormat(locale, { dateStyle: "long", timeZone: "UTC" }).format(ms);
}

/** The last day of grace, `grace_days` after `expires_at`. */
export function graceEnd(expires: string, days: number): string {
  const ms = Date.parse(`${expires}T00:00:00Z`) + days * 86_400_000;
  return Number.isNaN(ms) ? expires : new Date(ms).toISOString().slice(0, 10);
}
