// What a refused call says to the user, from its typed failure.
import {
  DesignerError,
  type CommandError,
  type DesignerFailure,
  type ImportError,
} from "@cavea/designer-core";
import { fields, formatBytes, msg, msgOf } from "../i18n/messages.ts";

const fieldName = (key: string): string => (fields as Record<string, string>)[key] ?? key;

export function commandMessage(e: CommandError): string {
  switch (e.code) {
    case "invalidValue":
      return msg("commandError.invalidValue", { field: fieldName(e.data) });
    case "notApplicable":
      return msg("commandError.notApplicable", { field: fieldName(e.data.field) });
    case "limit":
      return msg("commandError.limit", { what: e.data });
    case "duplicateCategory":
    case "missingCategory":
      return msg(`commandError.${e.code}`, { key: e.data });
    case "categoryInUse":
      return msg("commandError.categoryInUse", { key: e.data.key, count: e.data.references });
    default:
      return msg(`commandError.${e.code}`);
  }
}

export function importMessage(e: ImportError): string {
  switch (e.code) {
    case "tooLarge":
      return msg("importError.tooLarge", {
        bytes: formatBytes(e.bytes),
        limit: formatBytes(e.limit),
      });
    case "json":
      // serde's message ends with the position, which the message gives first.
      return msg("importError.json", {
        line: e.line,
        column: e.column,
        message: e.message.replace(/ at line \d+ column \d+$/, ""),
      });
    case "schema":
      return msg("importError.schema", { schema: e.schema });
    case "migration":
      return msg("importError.migration", { from: e.from, problem: e.problem });
    case "nulEscape":
      return msg("importError.nulEscape");
    case "duplicateId":
      return msg("importError.duplicateId", { id: e.id });
    case "limit":
      return msg("importError.limit", {
        what: msgOf("kinds", e.what) ?? e.what,
        count: e.count,
        limit: e.limit,
      });
    case "seatsPerRow":
      return msg("importError.seatsPerRow", { node: e.node, seats: e.seats, limit: e.limit });
  }
}

/** A failure other than a trap, in words. */
export function failureMessage(f: Exclude<DesignerFailure, { kind: "panic" }>): string {
  switch (f.kind) {
    case "command":
      return commandMessage(f.error);
    case "import":
      return importMessage(f.error);
    case "arguments":
      return msg("error.arguments", { message: f.message });
    default:
      return msg("error.generic", { message: f.kind });
  }
}

/** The failure a thrown value carries, if it is the designer's. */
export const failureOf = (error: unknown): DesignerFailure | null =>
  error instanceof DesignerError ? error.failure : null;
