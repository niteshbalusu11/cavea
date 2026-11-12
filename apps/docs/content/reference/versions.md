# Versions and upgrades

How Cavea is versioned, what may change between versions, which versions work together, and how to upgrade. By the end you will know which seat-map package works with which server, and what an upgrade does to your data.

## One version for everything

Cavea releases its parts together under one version number: the server image (`{{image}}`), the seat map (`{{renderer_package}}` and its script-tag build), the React component and the server SDK (`{{server_sdk_package}}`). Version `{{version}}` of each was made from the same source.

Versions follow [semantic versioning](https://semver.org): `MAJOR.MINOR.PATCH`.

| Part | What it means |
|---|---|
| Patch (`1.4.2` to `1.4.3`) | Fixes only. Nothing you call or receive changes shape. |
| Minor (`1.4` to `1.5`) | New features. Everything that worked keeps working. |
| Major (`1.x` to `2.0`) | May remove or change something. Every such change is listed under **Upgrading** in the [changelog](changelog.md). |

> **Note** Before 1.0, a minor version may also break something (`0.3` to `0.4`). The changelog lists every such change under **Upgrading**; read it before each upgrade.

A version with a suffix, such as `1.5.0-rc.1`, is a pre-release: for trying, not for production. npm installs it only when you ask for it (`{{renderer_package}}@next`).

## What stays compatible

| Surface | Promise |
|---|---|
| The server API | Its paths start with `/v1`. Within `/v1`, changes only add: new routes, new optional request fields, new fields in replies, new error reasons. Your code should ignore reply fields it does not know. A change that would break a client gets a new path prefix, and `/v1` stays. |
| Compiled charts | A newer server and seat map read every chart an older one compiled. |
| The seat map's live connection | The seat map and the server agree on the newest protocol both speak when they connect. |
| Seat map against server | You can upgrade the server and the seat map separately. A server supports the seat map of its own minor version and of the two minor versions before it: server `1.5` works with seat maps `1.3`, `1.4` and `1.5`. |
| The database | Upgrades change the database schema forward only. An older server refuses to start on a database a newer one has upgraded: it says so in its log and changes nothing. |

## Supported versions

The two newest minor versions receive security fixes. When `1.6` is released, `1.4` stops receiving them; upgrade to a supported version to get a fix.

## Upgrading the server

1. Read the version's section of the [changelog](changelog.md), and its **Upgrading** notes for every version between yours and the new one.
2. Back up the database.
3. Change the image's tag to the new version and restart the server: in Compose, set `image: {{image}}` and run `docker compose up -d`.
4. The server upgrades the database schema as it starts and logs `database schema up to date`. With several servers on one database, the first one to start does it and the others wait for it.
5. Once `/readyz` answers `ready`, holds, bookings and the console work as before: holds that were live keep their tokens and expiry, and people signed in to the console stay signed in.

An upgrade that cannot run while the server takes traffic says so under **Upgrading**, with what to do instead.

To go back to the older version, restore the backup taken in step 2: an older server does not start on an upgraded database.

## Upgrading the seat map

Install the new version of the package (`bun add {{renderer_package}}@{{version}}`, or `npm install`), or change the version in the script tag's URL, and deploy your page. Upgrade the server first: a seat map newer than the server is not supported.

## Checking what you run

- The server logs its version when it starts (`listening`, with `version`), and its API document says it: `GET /v1/openapi.json`, `info.version`.
- The image is labelled with its version and the commit it was built from: `docker image inspect {{image}}` (`org.opencontainers.image.version`, `org.opencontainers.image.revision`).
- To check that an image or a package is the one Cavea released, see [Verifying a release](verify-release.md).
