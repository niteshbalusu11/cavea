// A validation finding in a person's words (07 §10): what is wrong, naming
// things as the layers panel names them and rows and seats by the labels a
// buyer reads, and what to do about it. The edit core's view carries the
// particulars (`FindingDetail`) and the names; the labels of the row and
// seat a finding names are read for the findings shown only
// (`findingPlace`). No DOM, so it is tested alone.
import type { FindingPlace, FindingView, NodeKind } from "@cavea/designer-core";
import { formatNumber, msg, msgOf, type MessageId, type Params } from "../i18n/messages.ts";

/** A node as the layers panel names it (`rowName` there). */
export function nodeName(name: string | null, kind: NodeKind | null, id: number | null): string {
  if (name !== null) return name;
  if (kind === null || id === null) return msg("finding.something");
  return msg("layers.unnamed", { kind: msg(`kind.${kind}`), id: String(id) });
}

const ordinals = new Intl.PluralRules("en", { type: "ordinal" });
const SUFFIX: Record<string, string> = { one: "st", two: "nd", few: "rd", other: "th" };
/** 1st, 2nd, 3rd, 4th. */
const ordinal = (n: number) => `${formatNumber(n)}${SUFFIX[ordinals.select(n)] ?? "th"}`;

/** Where in `node` a row-level finding is: "Row C of Stalls", "The 3rd row
 * of Stalls" (an unlabelled one), or the free row itself. */
function rowWhere(f: FindingView, node: string, place: FindingPlace | null): string {
  if (f.kind === "row") return node;
  if (place?.row !== null && place?.row !== undefined) {
    return msg("finding.rowOf", { row: place.row, node });
  }
  if (place?.rowNumber !== null && place?.rowNumber !== undefined) {
    return msg("finding.rowNumberOf", { n: ordinal(place.rowNumber), node });
  }
  return msg("finding.aRowOf", { node });
}

/** Where a seat-level finding is: "Seat A-C-12 of Stalls" by its full
 * label, else by its place in its row. */
function seatWhere(f: FindingView, node: string, place: FindingPlace | null): string {
  if (place?.seat !== null && place?.seat !== undefined) {
    return msg("finding.seatOf", { seat: place.seat, node });
  }
  if (place?.seatNumber !== null && place?.seatNumber !== undefined) {
    const n = ordinal(place.seatNumber);
    const row = lower(rowWhere(f, node, place));
    return msg("finding.seatNumberOf", { n, row: f.kind === "row" ? node : row });
  }
  return msg("finding.aSeatOf", { node });
}

/** A document field's name as the inspector shows it, else in words. */
function fieldName(field: string): string {
  const last = field.split(".").at(-1) ?? field;
  return (
    msgOf("field", last) ??
    msgOf("findingField", last) ??
    last.replace(/([a-z])([A-Z])/g, "$1 $2").toLowerCase()
  );
}

/** A phrase's first letter in lower case, for running text. */
const lower = (s: string) => s.charAt(0).toLowerCase() + s.slice(1);

/** What the finding says, in full. `place` is `findingPlace`'s answer for
 * findings naming a row or seat. */
export function findingText(f: FindingView, place: FindingPlace | null = null): string {
  const node = nodeName(f.name, f.kind, f.node);
  const related = nodeName(f.relatedName, f.relatedKind, f.related);
  const say = (id: MessageId, params: Params = {}) =>
    msg(id, {
      node,
      related,
      row: rowWhere(f, node, place),
      seat: seatWhere(f, node, place),
      ...params,
    });
  const d = f.detail;
  switch (d.kind) {
    case "schema":
      return say("finding.schema", { schema: d.schema, newest: d.newest });
    case "migration":
      return say("finding.migration", { from: d.from });
    case "duplicateLabel":
      return say(f.related === f.node ? "finding.duplicateLabelSelf" : "finding.duplicateLabel", {
        label: d.label,
      });
    case "labelTooLong":
      return say("finding.labelTooLong");
    case "emptyLabel":
      return say(d.seat ? "finding.emptySeatLabel" : "finding.emptyRowLabel");
    case "noCategory":
      return say("finding.noCategory");
    case "unknownCategory":
      return say("finding.unknownCategory", { key: d.key });
    case "unknownFloor":
      return say("finding.unknownFloor");
    case "unknownIcon":
      return say("finding.unknownIcon", { key: d.key });
    case "floorConflict":
      return say("finding.floorConflict", {
        floor: d.sectionFloor ?? msg("finding.itsFloor"),
      });
    case "duplicateFloor":
      return say("finding.duplicateFloor", { floor: d.floor ?? msg("finding.aFloor") });
    case "duplicateCategory":
      return say("finding.duplicateCategory", { key: d.key });
    case "categoryColor":
      return say("finding.categoryColor", { category: d.category });
    case "focalPoint":
      return say("finding.focalPoint", { floor: d.floor ?? msg("finding.theFloor") });
    case "duplicateId":
      return say("finding.duplicateId");
    case "invalidValue":
      return say("finding.invalidValue", { field: fieldName(d.field) });
    case "emptyText":
      return say("finding.emptyText");
    case "outOfRange":
      return say("finding.outOfRange");
    case "geometry":
      return say("finding.geometry", {
        problem: msgOf("findingGeometry", d.problem.replaceAll(" ", "_")) ?? d.problem,
      });
    case "tooMany":
      return say(f.node === null ? "finding.tooManyChart" : "finding.tooMany", {
        limit: d.limit,
        what: d.what,
      });
    case "unsupported":
      return say(d.nodeKind === "section" ? "finding.nestedSection" : "finding.unsupported", {
        what: msgOf("kindOne", d.nodeKind) ?? d.nodeKind,
      });
    case "overlap":
      return say(f.related === f.node ? "finding.overlapSelf" : "finding.overlap");
    case "noFocalPoint":
      return d.floor === null
        ? say("finding.noFocalPoint")
        : say("finding.noFocalPointFloor", { floor: d.floor });
    case "singleSeat":
      return say("finding.singleSeat");
    case "outsideOutline":
      return say("finding.outsideOutline");
    case "unusedCategory":
      return say("finding.unusedCategory", { category: d.category });
    case "noAccessibleSeat":
      return say("finding.noAccessibleSeat");
    case "similarColors":
      return say("finding.similarColors", { category: d.category, other: d.other });
    case "lowContrast":
      return say("finding.lowContrast", {
        category: d.category,
        background: msgOf("findingBackground", d.background) ?? d.background,
        ratio: formatNumber(d.ratio),
      });
    case "textTooSmall":
      return say("finding.textTooSmall", { px: formatNumber(d.px) });
    case "noSectionLabel":
      return say("finding.noSectionLabel");
    case "chartTooLarge":
      return say("finding.chartTooLarge", { extent: d.extent });
    case "tooManySeatsInRow":
      return say("finding.tooManySeatsInRow", { seats: d.seats, limit: d.limit });
    case "tooManySeats":
      return say("finding.tooManySeats", { limit: d.limit });
    case "seatIds":
      return say("finding.seatIds");
    case "removedOutOfRange":
      return say("finding.removedOutOfRange");
    case "arcRow":
      return say("finding.arcRow");
    case "pathTooShort":
      return say("finding.pathTooShort");
    case "directionNotForRows":
      return say("finding.directionNotForRows");
    case "invalidLabelStart":
      return say("finding.invalidLabelStart");
    case "labels":
      return say("finding.labels", {
        problem: msgOf("findingLabels", d.problem) ?? d.problem,
      });
  }
}

/** Whether a finding names a row or seat, whose labels `findingPlace` reads. */
export const namesPlace = (f: FindingView): boolean =>
  f.node !== null && (f.row !== null || f.seat !== null);
