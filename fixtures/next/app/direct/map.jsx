"use client";
import { useEffect, useRef } from "react";

export default function Map() {
  const element = useRef(null);
  useEffect(() => {
    const controller = new AbortController();
    const errors = [];
    import("@cavea/renderer").then(({ createSeatMap, customState }) =>
      createSeatMap(element.current, {
        chart: { url: "/chart.ccf" },
        state: customState({
          connect: async () => ({ kind: "map", unavailable: [] }),
          disconnect: () => undefined,
        }),
        signal: controller.signal,
        onError: (error) => errors.push(error.code),
        onMetrics: (metrics) => {
          if (metrics.marks.first_frame !== undefined) {
            window.compat = { firstFrame: metrics.marks.first_frame, errors };
          }
        },
      }).catch((error) => {
        if (error.code !== "aborted") {
          window.compat = { failed: error.code, errors };
        }
      }),
    );
    return () => controller.abort();
  }, []);
  return <div id="map" ref={element} style={{ height: 400 }} />;
}
