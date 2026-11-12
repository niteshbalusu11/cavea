# Workspaces and roles

A workspace is a tenant of the server: its own charts, events, keys and members, unseen by every other workspace. This page explains workspaces, the people in them and what each role may do.

## Workspaces

Everything in Cavea belongs to one workspace. Every key acts in its own workspace only, and an identifier from another workspace is `404 not_found`, as if it did not exist.

A server's first workspace, named Main, is made when its owner account is created from the setup link. A platform that serves many organisers can give each its own workspace, created with the [admin key](keys.md#the-admin-key) ([create a workspace](../api/workspaces.md#createWorkspace)), which returns the workspace's first secret key.

## Members

People sign in to the console with an email and a password of 15 to 128 characters. An owner or admin invites others from **Settings**: the invitation is a link, valid for a limited time, that the person opens to create their account or to join with the one they have. Members, invitations and every change to them are in the workspace's audit log.

## Roles

Each member has one role in each workspace they belong to:

| Permission | Owner | Admin | Editor | Operator | Viewer |
|---|---|---|---|---|---|
| See charts and events (`charts.read`, `events.read`) | ✓ | ✓ | ✓ | ✓ | ✓ |
| Create, edit and publish charts; upload images (`charts.write`) | ✓ | ✓ | ✓ | | |
| Archive charts (`charts.archive`) | ✓ | ✓ | | | |
| Create, change and archive events (`events.write`) | ✓ | ✓ | | | |
| Hold seats, create and extend hold tokens (`seats.hold`) | ✓ | ✓ | | | |
| Block, release and book seats (`seats.write`) | ✓ | ✓ | | ✓ | |
| Manage API keys (`developers`) | ✓ | ✓ | | | |
| Manage members and the workspace (`settings`) | ✓ | ✓ | | | |
| Make or unmake owners (`ownership`) | ✓ | | | | |

So an editor draws and publishes charts but cannot sell; an operator at a box office books, blocks and releases seats but cannot change charts; a viewer looks. A request a role does not allow is `403 forbidden`, `reason` `role`, with `details.permission` naming what was missing. Every operation in the [API reference](../api/index.md) shows its permission and the roles that have it.

A workspace always keeps at least one owner.

## In the API

[Workspaces](../api/workspaces.md) (admin key) and [console sessions and members](../api/console.md).
