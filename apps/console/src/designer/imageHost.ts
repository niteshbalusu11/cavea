// The image tool in the page (07 §4, DSN-020): places a picked file as an
// image node at once, uploads it in the background for a server chart, and
// tells the canvas where each image's bytes are. One per designer page.
//
// - A server chart: the node is created with the file's content hash before
//   the upload ends, and drawn from the file kept in the page; a save waits
//   for the uploads (`settled`), so the server never holds a draft naming an
//   image it lacks. An upload with no answer is retried (as saves are); one
//   the server refuses removes the nodes naming it, as one undo step.
// - A local chart file (`/designer/local`): there is no server to hold the
//   picture and the chart document never holds image bytes, so the picture
//   is kept for this page only, and the person is told so. The file saved
//   names the picture by its hash, which an upload of the same file would
//   give again.
import type { Designer, ImageAsset, ImageRole } from "@cavea/designer-core";
import { createSignal, type Accessor } from "solid-js";
import { call } from "../api/client.ts";
import { asApiError } from "../api/query.ts";
import { describe } from "../console/errors.ts";
import { msg } from "../i18n/messages.ts";
import { notify } from "../ui/overlays.tsx";
import type { SaveReply } from "./autosave.ts";
import type { Editor } from "./editor.ts";
import {
  checkImage,
  MAX_SIDE,
  nodesNaming,
  Uploads,
  type Refusal,
  type UploadReply,
  type UploadSummary,
} from "./images.ts";
import { saveReplyOf, type ChartRef } from "./server.ts";

export interface ImageHost {
  /** No server: pictures are kept for this page only. */
  local: boolean;
  /** Where the canvas finds the picture of `hash`. */
  source(hash: string): ImageAsset | null;
  /** Pictures the canvas could not load. */
  failed: Accessor<ReadonlySet<string>>;
  markFailed(hash: string): void;
  /** Calls `run` when a picture that failed has bytes again (placed
   * again); returns the unsubscribe. */
  onAvailable(run: () => void): () => void;
  /** Uploads in progress or waiting; `null` for a local chart. */
  uploads: Accessor<UploadSummary | null>;
  /** Whether the upload of `hash` is still to finish. */
  pending(hash: string): boolean;
  /** Checks `file` and places it at the view's centre, selected. */
  place(editor: Editor, file: File, role?: ImageRole): Promise<boolean>;
  /** For a save: `null` once every upload is done, else what the save
   * reports meanwhile (`Uploads.settled`). Tries waiting uploads first. */
  settled(): Promise<SaveReply | null>;
  retry(): void;
  dispose(): void;
}

/** The public URL of an asset (08 §6.2): fetched, so `connect-src`. */
export const assetUrl = (hash: string): string => `/v1/public/assets/${hash}`;

const realClock = {
  now: () => Date.now(),
  set: (run: () => void, ms: number) => setTimeout(run, ms),
  clear: (handle: unknown) => clearTimeout(handle as ReturnType<typeof setTimeout>),
};

async function measure(blob: Blob): Promise<{ width: number; height: number }> {
  const bitmap = await createImageBitmap(blob);
  const size = { width: bitmap.width, height: bitmap.height };
  bitmap.close();
  return size;
}

/** A refusal in words. */
export function refusalMessage(r: Refusal): string {
  switch (r.kind) {
    case "tooLarge":
      return msg("image.tooLarge", { mb: Math.round(r.limit / (1024 * 1024)) });
    case "type":
      return msg("image.type");
    case "unreadable":
      return msg("image.unreadable");
    case "dimensions":
      return msg("image.dimensions", { px: r.limit });
  }
}

/** The server's refusal of an upload, in words (08 §6.2's reasons). */
function uploadReplyOf(e: unknown): UploadReply {
  const error = asApiError(e);
  const reply = saveReplyOf(e);
  if (reply.kind === "offline" || reply.kind === "signedOut") return reply;
  const limit = error.details["limit"];
  const reason: Record<string, string> = {
    asset_type: msg("image.type"),
    asset_malformed: msg("image.malformed"),
    asset_dimensions: msg("image.dimensions", { px: typeof limit === "number" ? limit : MAX_SIDE }),
    body_too_large: msg("image.tooLarge", {
      mb: typeof limit === "number" ? Math.round(limit / (1024 * 1024)) : 10,
    }),
  };
  return { kind: "refused", message: reason[error.reason ?? ""] ?? describe(error) };
}

export function createImageHost(props: {
  designer: Promise<Designer>;
  /** The server chart, or `null` for a local one. */
  server: ChartRef | null;
  /** The open editor, for removing a refused image. */
  editor: Accessor<Editor | null>;
}): ImageHost {
  const blobs = new Map<string, Blob>();
  const [failed, setFailed] = createSignal<ReadonlySet<string>>(new Set());
  const [summary, setSummary] = createSignal<UploadSummary | null>(null);
  const available = new Set<() => void>();
  const ref = props.server;
  const uploads =
    ref === null
      ? null
      : new Uploads({
          clock: realClock,
          async upload(blob) {
            try {
              const asset = await call(
                "uploadAsset",
                { body: blob },
                new AbortController().signal,
                { workspace: ref.workspace },
              );
              return { kind: "uploaded", hash: asset.hash };
            } catch (e) {
              return uploadReplyOf(e);
            }
          },
          onChange: setSummary,
          onRefused(hash, message) {
            notify(msg("image.refused", { message }), "error");
            const e = props.editor();
            const draft = e?.run((s) => s.export());
            if (e === undefined || e === null || draft === undefined) return;
            const ids = nodesNaming(draft, hash);
            if (ids.length > 0) e.run((s) => s.apply({ op: "deleteNodes", ids }));
          },
        });
  const online = () => uploads?.retryNow();
  window.addEventListener("online", online);

  return {
    local: ref === null,
    source(hash) {
      return blobs.get(hash) ?? (ref === null ? null : assetUrl(hash));
    },
    failed,
    markFailed(hash) {
      setFailed((s) => new Set([...s, hash]));
    },
    onAvailable(run) {
      available.add(run);
      return () => available.delete(run);
    },
    uploads: () => (uploads === null ? null : summary()),
    pending(hash) {
      const s = uploads?.state(hash) ?? null;
      return s === "sending" || s === "waiting";
    },
    async place(editor, file, role = "reference") {
      const designer = await props.designer;
      const checked = await checkImage(file, { measure, hash: (b) => designer.assetHash(b) });
      if ("refused" in checked) {
        notify(refusalMessage(checked.refused), "error");
        return false;
      }
      const { blob, hash, width, height } = checked.ok;
      blobs.set(hash, blob);
      if (failed().has(hash)) {
        setFailed((s) => {
          const next = new Set(s);
          next.delete(hash);
          return next;
        });
        // The canvas asks for it again, from these bytes.
        for (const run of available) run();
      }
      const c = editor.camera();
      const id = editor.run((s) =>
        s.insert(null, {
          kind: "image",
          center: [c.x, c.y],
          size: [width, height],
          asset: hash,
          role,
        }),
      );
      if (id === undefined) return false;
      uploads?.add(hash, blob);
      // A picture larger than the view is framed whole.
      const v = editor.view();
      if (width * v.zoom > v.width || height * v.zoom > v.height) {
        editor.frame({
          min: [c.x - width / 2, c.y - height / 2],
          max: [c.x + width / 2, c.y + height / 2],
        });
      }
      editor.reveal(id);
      if (ref === null) notify(msg("image.sessionOnly"));
      return true;
    },
    async settled() {
      uploads?.retryNow();
      return (await uploads?.settled()) ?? null;
    },
    retry: online,
    dispose() {
      window.removeEventListener("online", online);
      uploads?.dispose();
    },
  };
}
