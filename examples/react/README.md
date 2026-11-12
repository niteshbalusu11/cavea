# Cavea example: React

A small Vite and React app with the seat map: `<SeatMap>` from `@cavea/react`, the buyer's selection lifted into the app's state and shown by a component of its own, prices from the app's table, and the map in your brand's colours with a light and dark switch.

## What you need

- Node.js 20.19 or later. npm comes with it; bun, pnpm and yarn work too.
- A Cavea server with a published chart, an event and a publishable key: the [quick start](https://github.com/niteshbalusu11/cavea/blob/main/apps/docs/content/quick-start.md) makes all three.

## Run it

```sh
npm install
```

Set three environment variables, for example in a file `.env.local` (Vite reads it; it is public, since Vite builds these values into the page):

| Variable | What it is |
|---|---|
| `VITE_CAVEA_URL` | Your Cavea server, such as `http://localhost:8080` |
| `VITE_CAVEA_PUBLISHABLE_KEY` | A publishable key, starting `pk_`: safe in a page, it can only show and hold seats |
| `VITE_CAVEA_EVENT` | The event's key, such as `opening-night` |

```sh
npm run dev
```

Open the address it prints. Choose seats: each is held for you, and the list under the map follows. `npm run build` type-checks and writes `dist/`.

The prices in `src/App.tsx` are for the sample theatre's category keys; change them to your chart's.

## What to look at

- `src/App.tsx`: the adapter made once with `useMemo` (a new adapter makes a new map), the selection in `useState`, and the theme: tokens over a base, changed on the live map.
- `src/Summary.tsx`: a component that only reads the selection.

Checkout needs a backend: the [Next.js example](../nextjs) has one.

## Read more

- [React](https://github.com/niteshbalusu11/cavea/blob/main/apps/docs/content/guides/react.md): how `<SeatMap>`'s props behave across renders.
- [Theming](https://github.com/niteshbalusu11/cavea/blob/main/apps/docs/content/renderer/theming.md): every colour token.

## Before the packages are published

`@cavea/react` and `@cavea/renderer` are not on npm yet. Until they are, build them from the Cavea repository and install the tarballs in place of the versions in `package.json`: [Before the first release](https://github.com/niteshbalusu11/cavea/blob/main/apps/docs/content/before-release.md#the-npm-packages).
