# Quick start

At the end of this page you will have the Cavea server running on your own machine, a theatre's seat map made in its console from a template, and a web page on which clicking a seat holds it for you on the server. It takes about ten minutes.

You need Docker with Docker Compose, a terminal and a web browser. Everything runs in containers, and the last step removes them.

On the way you copy two values from one place to another: the **setup link** (step 2) and the **publishable key** (step 6). Both are called out where they appear.

## 1. Start the server

Make a directory for the quick start, with a `public` directory inside it for your web page, and go into it:

```sh qs=dir
mkdir -p cavea-quickstart/public && cd cavea-quickstart
```

Save this as `compose.yaml` in that directory. It runs three containers: PostgreSQL, the Cavea server, and a small web server for the page you write in step 7.

```yaml file=compose.yaml qs=file
# The Cavea quick start: for trying Cavea on this machine only.
# Every port is published on 127.0.0.1, so nothing is reachable from outside.
name: cavea-quickstart

services:
  postgres:
    image: postgres:18-alpine
    environment:
      POSTGRES_USER: cavea
      POSTGRES_PASSWORD: quickstart-only
      POSTGRES_DB: cavea
    volumes:
      - postgres-data:/var/lib/postgresql
    healthcheck:
      # Over TCP, as the server connects: on its first start PostgreSQL runs
      # a set-up server that answers on its socket only, then restarts.
      test: ["CMD-SHELL", "pg_isready -h 127.0.0.1 -U cavea -d cavea"]
      interval: 2s
      retries: 30

  server:
    image: {{image}}
    depends_on:
      postgres:
        condition: service_healthy
    environment:
      CAVEA_DATABASE_URL: postgres://cavea:quickstart-only@postgres:5432/cavea
      CAVEA_PUBLIC_URL: http://localhost:8080
    ports:
      - "127.0.0.1:8080:8080"

  web:
    image: nginx:1.29-alpine
    volumes:
      - ./public:/usr/share/nginx/html:ro
    ports:
      - "127.0.0.1:8000:80"

volumes:
  postgres-data:
```

Start it:

```sh qs=up
docker compose up -d --wait
```

The first run downloads the three images, about 400 MB once unpacked. The command returns once every container is up; its last lines say so:

```text
 Container cavea-quickstart-web-1  Healthy
 Container cavea-quickstart-postgres-1  Healthy
 Container cavea-quickstart-server-1  Healthy
```

The console and the API are now at `http://localhost:8080`, and the web server at `http://localhost:8000`. If either port is taken on your machine, change the first number of its `ports` line, and the same number everywhere else on this page.

> **Note** This set-up is for your own machine: a fixed database password, plain HTTP, no backups. Running Cavea for real is a different set-up.

## 2. Create the owner account

While the server has no user, it writes a one-time **setup link** to its log. Print it:

```sh qs=link
docker compose logs server | grep -o 'http://localhost:8080/console/setup#token=[0-9a-f]*'
```

```text
http://localhost:8080/console/setup#token=0123456789abcdef…
```

On Windows PowerShell, use `docker compose logs server | Select-String setup_link` and copy the link from the line it prints.

**Copy the link into your browser.** It is valid for an hour; restarting the server makes a new one while it has no user.

On **Set up Cavea**, enter an email and a password of at least 15 characters, and choose **Create the owner account**. You are signed in, as the owner of a workspace named Main, and the console shows **Charts**.

<details class="more">
<summary>Prefer the API? Steps 3 to 6 with curl instead</summary>

Do this instead of steps 3 to 6, then go on with step 7. It uses a sample chart document, the console's theatre template as a file, so it makes the same chart of 561 seats.

1. In the console, open **Developers**, choose **New API key**, name it `Quick start backend`, choose **Secret key** and **Create key**. Copy the key it shows (it starts `sk_`) and keep it in your terminal:

   ```sh
   export CAVEA_SECRET_KEY='sk_paste-the-secret-key-here'
   ```

   A secret key has full power over the workspace: keep it out of web pages, and out of your shell history on a shared machine.

2. Download the sample chart document [theatre.json](samples/theatre.json) into the `cavea-quickstart` directory.

3. Create a chart from it, publish the chart, create an event on it and create a publishable key:

   ```sh qs=api
   curl -sS -X POST http://localhost:8080/v1/charts \
     -H "Authorization: Bearer $CAVEA_SECRET_KEY" -H 'Content-Type: application/json' \
     -d "{\"key\": \"grand-hall\", \"name\": \"Grand Hall\", \"document\": $(cat theatre.json)}"

   curl -sS -X POST http://localhost:8080/v1/charts/grand-hall/publish \
     -H "Authorization: Bearer $CAVEA_SECRET_KEY"

   curl -sS -X POST http://localhost:8080/v1/events \
     -H "Authorization: Bearer $CAVEA_SECRET_KEY" -H 'Content-Type: application/json' \
     -d '{"key": "opening-night", "name": "Opening Night", "chart": "grand-hall"}'

   curl -sS -X POST http://localhost:8080/v1/keys \
     -H "Authorization: Bearer $CAVEA_SECRET_KEY" -H 'Content-Type: application/json' \
     -d '{"kind": "publishable", "name": "Buyer page"}'
   ```

   Each prints the JSON of what it made. The last one's `key` is the **publishable key** (it starts `pk_`): copy it for step 7.

The calls are in the [server API reference](api/index.md): [create a chart](api/charts.md#createChart), [publish](api/charts.md#publishChart), [create an event](api/events.md#createEvent), [create a key](api/keys.md#createKey).

</details>

## 3. Create a chart

A [chart](concepts/charts.md) is a venue's seat map. Start one from the theatre template:

1. On **Charts**, choose **New chart**. Enter the name `Grand Hall`; its key fills in as `grand-hall`.
2. For **Start from**, choose **Theatre template (561 seats)**, then **Create chart**. The chart appears in the list.

## 4. Publish the chart

Events are sold from a published version of a chart, which never changes once stored. You publish in the designer, where the chart is drawn and edited.

1. Choose **Grand Hall** in the list, then **Open in designer**. The designer opens with a short tour: choose **Skip tour** (the tour has you draw a block of seats, which would change this chart; see it later in [the designer guide](designer/start.md#the-tour)). The canvas shows the theatre: three sections of stalls facing a stage, and the status bar at the bottom reads **561 seats**.
2. Choose **Publish** at the top right, then **Publish** in the dialog. It reports **Published version 1: 561 seats.** Choose **Close**.

![The designer showing the theatre template: three sections of curved rows facing the stage](images/designer.webp)

The template is an ordinary chart: anything in it can be changed in the designer, which saves as you go, and published again as version 2. See [Drawing a seating chart](designer/index.md).

## 5. Create an event

An [event](concepts/events.md) is one performance: it keeps its own seat state on one version of a chart.

1. Open the designer's **Menu** (top left) and choose **Back to console**.
2. Open **Events** and choose **New event**.
3. Enter the name `Opening Night`. Its key fills in as `opening-night`: the page in step 7 names the event by this key, so keep it as it is.
4. For **Chart**, choose **Grand Hall (grand-hall)**. The **Chart version** is version 1, the published one.
5. Choose **Create event**.

## 6. Create a publishable key

A [publishable key](concepts/keys.md#publishable-keys) lets a web page show the event's seats and hold them, and nothing else, so it is safe to put in a page.

1. Open **Developers** and choose **New API key**.
2. Name it `Buyer page` and choose **Publishable key**. Leave **Allowed origins** empty, which accepts any page.
3. Choose **Create key**. The key is shown once, starting `pk_`: **copy the publishable key** now.

## 7. Write the page

Save this as `public/index.html`, with your publishable key in place of `pk_paste-your-publishable-key-here`:

```html file=public/index.html qs=file
<!doctype html>
<html lang="en">
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Opening Night: choose your seats</title>
<h1>Opening Night</h1>
<div id="map" style="height: 70vh"></div>
<p id="selection">Click a seat to hold it.</p>
<script src="{{renderer_script}}"></script>
<script>
  Cavea.createSeatMap(document.getElementById("map"), {
    state: Cavea.caveaServer({
      baseUrl: "http://localhost:8080",
      publishableKey: "pk_paste-your-publishable-key-here",
      event: "opening-night",
    }),
    onSelectionChange: (selection) => {
      const seats = selection.objects.map((seat) => seat.label).join(", ");
      document.getElementById("selection").textContent = seats
        ? "Held for you: " + seats
        : "Click a seat to hold it.";
    },
  });
</script>
```

The script tag loads the seat map, `{{renderer_package}}`, which sets `window.Cavea`. `caveaServer` connects the map to your server: it reads the event's seats, keeps them current as others take them, and holds a seat when the buyer clicks it. The web server in `compose.yaml` serves the `public` directory as it is, so there is nothing to build.

## 8. Hold a seat

Open `http://localhost:8000`. The map shows the event's 561 seats in their three sections, facing the stage. Zoom in with the + button at the map's top right (or the mouse wheel) until the seats are easy to point at, and click one: it changes to a check mark, and the line under the map says which seat is held for you, such as `Held for you: Stalls Centre-C-108`.

![The buyer's page, zoomed in: seat 108 in row C of Stalls Centre is selected and held, and the line under the map says which](images/buyer-map.webp)

The label is the seat's name in the chart: section, row and seat, joined by `-`. The tooltip shows the same three parts.

The seat is now held on the server under a [hold token](concepts/holds.md) that the map created for this browser tab. To see what others see:

- Open the same address in a private window: the seat is drawn unavailable there, and it changes back if you click it again in the first window, which releases it.
- In the console, open **Events** and choose **Opening Night**: the event manager counts the seat as held.

![The event manager: 0 booked, 1 held, 0 blocked, 560 free, with the held seat drawn unavailable](images/event-manager.webp)

The hold lasts 15 minutes, the event's hold lifetime, and then the seat is free again. In a real sale your backend takes payment and then books what the hold token holds, with a secret key: see [Bookings](concepts/bookings.md).

> **Note** This server runs without a licence file, so it reports itself as unlicensed: the console says so in one line, and the seat map shows a small "Development licence" mark in a corner. Holding and booking work the same in every licence state ([the licence](deployment/licence.md)).

## 9. Clean up

Stop the containers and delete their data:

```sh qs=down
docker compose down -v
```

## Next

- [Examples](examples/index.md): this page grown into a checkout, in plain HTML, React, Next.js and Node.
- [Drawing a seating chart](designer/index.md): the designer, task by task, for whoever draws your venues.
- [Concepts](concepts/index.md): charts, events, holds, bookings, keys and the realtime stream, each on a page.
- [Server API](api/index.md): every operation, with its credential, body and replies.
- [Error codes](reference/errors.md) and [limits](reference/limits.md).
