# Cavea documentation

Cavea is a seat-map toolkit for ticketing platforms. It gives you a seat picker for your buyers' web pages, a designer in which venue staff draw seat maps, and a server that you host, which keeps the state of every seat of every event and holds and books seats for you.

Your platform keeps what it already owns: buyers, prices, payments, orders. Cavea knows seats.

## What is in it

| Part | What it does |
|---|---|
| **Seat map** (`{{renderer_package}}`) | Draws a venue in the buyer's browser with WebGL, live: seats others take show as taken within moments. Buyers zoom, pan and pick seats with a pointer, a finger or the keyboard. A script tag, an ES module, or a React component. |
| **Server** (a container image) | Holds the seat state of your events in PostgreSQL. Holds seats for buyers, books them when your backend says so, frees them when holds expire, and streams changes to every open seat map. One container and a PostgreSQL database. |
| **Console** (in the server) | Where staff draw and publish charts, create events, watch and change seats in the event manager, and manage API keys, members and roles. |
| **Server SDK** (`{{server_sdk_package}}`) | Calls the server from a Node backend: book, release, manage events, sign session tokens. |

## Where to start

- **Trying it:** the [quick start](quick-start.md) runs the server on your machine and gets a seat held in a web page in about ten minutes.
- **Understanding it:** [Concepts](concepts/index.md) explains charts, events, holds, bookings and keys, a page each.
- **Building on it:** the [server API](api/index.md) lists every operation; [error codes](reference/errors.md) and [limits](reference/limits.md) say what can go wrong and how far you can go.

## What is not here yet

Cavea is in early development and not yet released. These parts of the documentation come next: the seat map's full reference (options, callbacks, theming, React), guides for the hold, pay and book flow and for using your own backend, deploying the server for real, the designer's user guide, and example projects.
