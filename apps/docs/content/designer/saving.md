# Saving and working together

A chart in the console saves itself while you work: there is no save button to remember. This page says what the top bar tells you about saving, what happens when your connection drops, and what happens when a colleague opens the chart while you have it open.

## What the top bar says

Beside the undo arrows, the top bar says where your work is:

| It says | What it means | What to do |
|---|---|---|
| **Saving…** | Your latest changes are on their way to the server | Nothing |
| "Saved" and a time | Everything you see is saved on the server | Nothing |
| **Offline — changes kept locally** | The server cannot be reached. Your changes are kept in this browser and sent when the connection comes back | Keep working, or close the tab: the changes are offered when you open the chart again in this browser |
| **Offline — this browser cannot keep the changes: keep this tab open** | As above, but this browser cannot store them (a private window, or a full disk) | Keep the tab open until it says **Saved** |
| **Signed out — changes kept locally** | Your session ended | Sign in again; the changes are offered when the chart opens |
| **Not saved: changed elsewhere** | Someone saved the chart since you opened it (see below) | Choose **Resolve…** |
| "Not saved" and a reason | The server refused the save | Read the reason; **Retry** tries again |
| "Read-only" | You can look but not change: your role is viewer, or someone else is editing | See [someone else is editing](#someone-else-is-editing) |

![The top bar saying Saved, with the time](../images/designer-saved.webp)

![The top bar saying Offline — changes kept locally](../images/designer-offline.webp)

`Ctrl`+`S` (**Save now**) sends your changes at once instead of a moment later. To keep a copy of the chart as a file, open the **Menu** and choose **Export chart file**.

## If your changes did not reach the server

If the browser or the computer closed before your last changes were saved, the designer offers them the next time you open the chart in the same browser:

![The Unsaved changes found message, with Download mine, Keep the server’s draft and Restore my changes](../images/designer-recovery.webp)

- **Restore my changes** puts them back and saves them.
- **Keep the server’s draft** throws them away and opens the chart as it was last saved.
- **Download mine** saves them as a file first, so you can decide later.

If someone saved the chart since, the message says so: restoring then replaces their newer work.

## Someone else is editing

Only one person can change a chart at a time. If a colleague has it open, it opens read-only for you, and the top bar says who: "Read-only: box-office@example.org is editing (since 10:42)". You can still look around, zoom, and open the panels.

![The top bar saying the chart is read-only because a colleague is editing it](../images/designer-read-only.webp)

To edit it anyway, for example because your colleague went home and left it open:

1. Choose **Take over…** in the top bar.
2. Read the message and choose **Take over**.

Your colleague's designer turns read-only at once, and they are told who took over:

![The message a person sees when someone takes over: their unsaved changes are kept in their browser](../images/designer-taken-over.webp)

Changes they had not saved are not lost: they stay in their browser, and are offered to them (as above) when they take over again. Agree with your colleague which version to keep before either of you restores anything.

If your own designer says **Not saved: changed elsewhere**, someone saved the chart from another tab or browser while yours was not looking. Choose **Resolve…**, then **Keep mine (replaces theirs)**, **Open theirs (discards mine)**, or **Download mine** to decide later.

> **Note** The same person signed in on two computers counts as one person: the second computer is not made read-only, and whichever saves second gets **Not saved: changed elsewhere**. Close the chart on one computer before you open it on another.
