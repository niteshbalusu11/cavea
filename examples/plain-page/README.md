# Cavea example: a plain page

One HTML file, `index.html`, with the seat map from a script tag: no build, no framework. The buyer chooses seats on the map, each one held for them on your Cavea server; under the map they see their seats with prices from the page's own price table, and a **Continue** button that sends the hold token to your backend.

## What you need

- A Cavea server with a published chart, an event and a publishable key: the [quick start](https://github.com/niteshbalusu11/cavea/blob/main/apps/docs/content/quick-start.md) makes all three.
- Node.js 20.11 or later, only for `serve.js`, a static file server for trying the page. Any web server works: the page is a static file.

## Run it

1. In `index.html`, set `CAVEA_URL` to your server, `PUBLISHABLE_KEY` to your publishable key (it starts `pk_` and is safe in a page) and `EVENT` to the event's key.
2. If your chart's category keys are not those of the sample theatre (`premium`, `standard`, `value`, `access`), change `PRICES` to yours.
3. Serve the directory and open it:

   ```sh
   npm start
   ```

   Open `http://localhost:8000` (`PORT=8001 npm start` serves on another port). Choose a seat: it is held for you, and listed under the map with its price.

**Continue** posts `{ holdToken }` to `/api/checkout` on the page's own origin. This example has no backend, so it says checkout is not available: your backend answers it, as the [Node backend example](../node-backend) does, by pricing what the hold token holds, taking payment and booking.

## Read more

- [Hold, pay, book](https://github.com/niteshbalusu11/cavea/blob/main/apps/docs/content/guides/hold-pay-book.md): the checkout behind **Continue**.
- [Options](https://github.com/niteshbalusu11/cavea/blob/main/apps/docs/content/renderer/options.md) and [callbacks](https://github.com/niteshbalusu11/cavea/blob/main/apps/docs/content/renderer/callbacks.md) of the seat map.

## Before the packages are published

The script URL in `index.html` is the published package's, on a CDN, and works once `@cavea/renderer` is released. Until then, build the script from the Cavea repository and serve it beside the page: [Before the first release](https://github.com/niteshbalusu11/cavea/blob/main/apps/docs/content/before-release.md#build-the-seat-map).
