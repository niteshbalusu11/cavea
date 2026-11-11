"use client";
// The client component: `<SeatMap>` with an adapter made once, reporting to
// `window.compat` for `fixtures/run.ts`.
import { SeatMap } from "@cavea/react";
import { customState } from "@cavea/renderer";
import { useMemo } from "react";

export default function Map() {
  const state = useMemo(
    () =>
      customState({
        connect: async () => ({ kind: "map", unavailable: [] }),
        disconnect: () => undefined,
      }),
    [],
  );
  const errors = useMemo(() => [], []);
  return (
    <SeatMap
      id="map"
      style={{ height: 400 }}
      chart={{ url: "/chart.ccf" }}
      state={state}
      onError={(error) => errors.push(error.code)}
      onMetrics={(metrics) => {
        if (metrics.marks.first_frame !== undefined) {
          window.compat = { firstFrame: metrics.marks.first_frame, errors };
        }
      }}
    />
  );
}
