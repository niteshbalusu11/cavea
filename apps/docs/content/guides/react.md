# React

This guide puts the seat map in a React application with `@cavea/react`, a component, `<SeatMap>`, over `{{renderer_package}}`. By the end you will know how its props behave across renders, how to reach the map's methods, and how to use it in Next.js.

## Install

```sh
npm install @cavea/react {{renderer_package}}
```

React 18 and 19 are supported. pnpm, yarn and bun work the same way.

> **Not yet** Neither package is published. Until they are, build them from the repository: [Before the first release](../before-release.md).

## A seat picker

```tsx check=types
import { SeatMap } from "@cavea/react";
import { caveaServer, type Selection } from "{{renderer_package}}";
import { useMemo, useState } from "react";

export function SeatPicker({ event }: { event: string }) {
  // Made once per event: a new adapter would recreate the map.
  const state = useMemo(
    () =>
      caveaServer({
        baseUrl: "https://seats.example.com",
        publishableKey: "pk_paste-your-publishable-key-here",
        event,
      }),
    [event],
  );
  const [selection, setSelection] = useState<Selection | null>(null);

  return (
    <>
      <SeatMap
        state={state}
        selection={{ max: 6 }}
        theme="light"
        onSelectionChange={setSelection}
        style={{ height: 520 }}
      />
      <p>{selection?.objects.map((seat) => seat.label).join(", ") || "Choose your seats."}</p>
    </>
  );
}
```

Every [option](../renderer/options.md) and [callback](../renderer/callbacks.md) of `createSeatMap` is a prop, except `signal`: the component's mount is the map's lifetime. Three props are the component's own:

<!-- members: react:SeatMapProps -->

## How props behave

- **The container.** The component renders one empty `<div>`, on the server too; the map is created in an effect after mount, inside it. Give the `div` a height through `style` or `className`.
- **Live props.** `pricing`, `theme` and `locale` are applied to the live map when they change in value: a new object or array with the same content is not a change. Removed, they go back to the default.
- **Props read at creation.** Every other option applies when the map is created; a later change is ignored, with a warning in a development build. `initialView` is meant for creation and does not warn.
- **Callbacks** can be new functions on every render: the map always calls the latest. The exceptions are `onMetrics`, `onError` and `formatPrice`, whose presence changes what the map does: they count from creation, and adding or removing one later warns.
- **A new `state` adapter**, or a `chart` with another URL or other bytes, destroys the map and creates another. Make the adapter with `useMemo`, as above, never in render: a new adapter on three renders in a row warns.
- **One adapter serves one map.** If a parent unmounts and mounts the component again with the same adapter object (React's `<Activity>` hiding and showing it, for example), key the `useMemo` on whatever remounts it, so the new map gets a new adapter. React's Strict Mode needs nothing: its extra mount is cancelled before anything is fetched.
- **Unmounting** destroys the map; one still loading is cancelled. The buyer's holds stay until they expire or your backend books or releases them.

## Calling methods

The ref is the map once it is ready, and `null` before, while it is recreated, and after unmount:

```tsx check=types
import { SeatMap, type SeatMapInstance } from "@cavea/react";
import { staticState } from "{{renderer_package}}";
import { useMemo, useRef } from "react";

export function Venue() {
  const map = useRef<SeatMapInstance>(null);
  const state = useMemo(() => staticState(), []);
  return (
    <>
      <SeatMap ref={map} chart={{ url: "/charts/grand-hall.ccf" }} state={state} style={{ height: 480 }} />
      <button type="button" onClick={() => map.current?.zoomTo("Stalls")}>
        Show the stalls
      </button>
    </>
  );
}
```

`SeatMapInstance` is the same type as `SeatMap` in `{{renderer_package}}`: its [methods](../renderer/methods.md).

## Errors

A failure after the map is ready goes to `onError`. A start that fails (a fatal error) also goes to `onError`; without `onError` it is thrown as an unhandled rejection, which a development build shows in the console. Cancelling by unmounting is not reported.

## Next.js

The seat map draws in the browser only, so it goes in a client component; the page around it can stay a server component. Nothing else is needed: `@cavea/react` renders an empty `div` on the server and creates the map after hydration, and Next.js copies the WebAssembly and the atlas into its build.

```tsx check=types
"use client";
import { SeatMap } from "@cavea/react";
import { caveaServer } from "{{renderer_package}}";
import { useMemo } from "react";

export default function SeatPicker({ event }: { event: string }) {
  const state = useMemo(
    () =>
      caveaServer({
        baseUrl: process.env.NEXT_PUBLIC_CAVEA_URL ?? "https://seats.example.com",
        publishableKey: process.env.NEXT_PUBLIC_CAVEA_PUBLISHABLE_KEY ?? "",
        event,
      }),
    [event],
  );
  return <SeatMap state={state} style={{ height: 520 }} />;
}
```

Save it as, say, `app/events/[event]/seat-picker.tsx` and render `<SeatPicker event="opening-night" />` from the server component page. The publishable key is safe in the browser (`NEXT_PUBLIC_…`); the secret key belongs only in route handlers and server actions, with `{{server_sdk_package}}` ([hold, pay, book](hold-pay-book.md)).

Without `@cavea/react`, import `{{renderer_package}}` inside an effect with `import()`, so the server never evaluates it, create the map there, and abort its `signal` in the effect's cleanup.

The [examples](../examples/index.md) include a React app with Vite (the selection lifted into the app, theming) and a Next.js app with the whole checkout in route handlers.

This set-up (App Router with Turbopack: a server component page holding a client component that uses `<SeatMap>`, and one that imports the renderer in an effect) is built, rendered on the server and hydrated in a browser by the repository's compatibility checks.
