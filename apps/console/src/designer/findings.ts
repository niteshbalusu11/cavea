// A validation finding in a person's words (07 §10): what is wrong, naming
// things as the layers panel names them and rows and seats by the labels a
// buyer reads, and what to do about it. The edit core's view carries the
// particulars (`FindingDetail`) and the names; the labels of the row and
// seat a finding names are read for the findings shown only
// (`findingPlace`). A publish the server refuses lists its findings the
// same way (`refused`). No DOM, so it is tested alone.
import type { FindingPlace, FindingView, Session } from "@cavea/designer-core";
import type { Finding } from "../api/generated.ts";
import { formatNumber, msg, msgOf, type MessageId, type Params } from "../i18n/messages.ts";

/** A node as the layers panel names it: the edit core names every node
 * (`naming.rs`), so only a finding without one, or of a node the chart no
 * longer holds, gets "An object". */
export const nodeName = (name: string | null): string => name ?? msg("finding.something");

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
  const node = nodeName(f.name);
  const related = nodeName(f.relatedName);
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

/** A finding as the dialogs show it: the catalogue's title for its code over
 * its sentence. */
export interface Said {
  code: string;
  title: string;
  text: string;
}

/** The catalogue's title for a code, else a plain one. */
export const findingTitle = (code: string): string =>
  msgOf("validation", code) ?? msg("finding.otherTitle");

/** One finding's identity, the same on both sides: code, nodes, words. */
const key = (f: Finding | FindingView) =>
  `${f.code}\u0000${f.node}\u0000${f.related}\u0000${f.message}`;

/** What the publish dialog needs of the open chart's session. */
export type Chart = Pick<Session, "validation" | "findingPlace" | "nodeName">;

/** The errors of a publish the server refused (07 §11), each in the
 * validation panel's words: the server compiled the draft the designer
 * holds, so a finding of the open chart's own validation with the same
 * code, nodes and compiler message is the same finding, said as the panel
 * says it. One the designer does not have (an image whose picture this
 * workspace lacks, which only the server checks; or a chart edited since)
 * gets its code's title and a sentence naming its object as the layers
 * panel does. Every finding is said, none by an internal id. */
export function refused(findings: Finding[], chart: Chart | null): Said[] {
  const errors = findings.filter((f) => f.severity === "error");
  const shown = errors.length > 0 ? errors : findings;
  const own = new Map<string, FindingView[]>();
  for (const f of chart?.validation().findings ?? []) {
    const k = key(f);
    own.set(k, [...(own.get(k) ?? []), f]);
  }
  return shown.map((f) => {
    const mine = own.get(key(f))?.shift();
    if (mine !== undefined) {
      const at = mine.node;
      const place =
        at !== null && namesPlace(mine)
          ? (chart?.findingPlace(at, mine.row, mine.seat) ?? null)
          : null;
      return { code: f.code, title: findingTitle(f.code), text: findingText(mine, place) };
    }
    const named = f.node === null ? null : (chart?.nodeName(f.node) ?? null);
    const node = nodeName(named?.name ?? null);
    const text =
      f.node === null
        ? msg("finding.otherChart")
        : f.code === "E007" && named?.kind === "image"
          ? msg("finding.missingPicture", { node })
          : msg("finding.other", { node });
    return { code: f.code, title: findingTitle(f.code), text };
  });
}
