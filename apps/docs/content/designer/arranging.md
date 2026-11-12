# Arrange the chart

At the end of this page your blocks will be in named sections, the stage and other landmarks will be drawn, and everything will be lined up.

## Sections

A section is a part of the house with a name buyers know: "Stalls", "Circle", "Box A". Its label starts every seat's label ("Stalls-C-12"), and buyers see it on the map. A block outside any section works too: its seats' labels are the row and the seat alone, "C-12" ([The full label](numbering.md#the-full-label)).

1. Open the **Layers** tab in the panels.
2. Right-click the block's line in the list and choose **Group into section**. The block is now inside a new section, which is selected.
3. In the **Inspector**, type the section's **Label** (such as "Stalls") and press `Enter`.

To put several blocks in one section, select them all first (hold `Shift` and click each in the list), then right-click one of them. Or choose the **Section** tool (`S`) and drag a box around the blocks on the canvas: every block wholly inside the box goes into the new section.

![The chart with its stalls and two boxes lined up; in the layers list, each block in its section and the floor plan locked and hidden](../images/designer-arranged.webp)

The section's other fields:

- **Category**: the category its blocks take when theirs is **Inherit** ([Categories](categories.md)).
- **Priority**: how soon the section's seats come up when the best available seats are picked for a buyer. 1 is normal; below 1 brings the section forward, above 1 holds it back. It needs a focal point ([No focal point](checking.md#no-focal-point)).
- **Entrance**: a note for buyers, such as "Door 3".
- **Outline**: the line drawn round the section on the buyer's map. **Automatic** follows its seats. **Make editable** turns it into points you can drag.

To take blocks out of a section, select it and choose **Dissolve** in the inspector. The blocks stay where they are.

## Select

- Click something to select it; `Shift`-click to add to the selection. Drag a box across empty canvas to select everything wholly inside it. `Ctrl`+`A` selects everything.
- Click in the **Layers** list to select there; this is the easiest way to select something under something else, or something small.
- In a section, the first click selects the section; double-click to reach the block inside. `Escape` steps back out.
- The status bar says how many things are selected.

## Move, turn and copy

- Move: drag the selection, or press the arrow keys (`Shift` and an arrow moves ten times as far). For an exact place, type **X** and **Y** in the inspector.
- Turn: drag the small circle above the selection, or type **Rotation** in the inspector. Hold `Shift` to turn in steps of 15 degrees.
- Copy: `Ctrl`+`D` (or **Duplicate** in the layers list's right-click menu) makes a copy beside the original and selects it. Move it into place.

A copy has the same labels as the original. The **Validation** panel reports each of its seats as a **Duplicate label** until you change the copy's section label or its numbering ([Check the chart](checking.md#duplicate-label)).

## Line things up

Select two or more objects, press `Ctrl`+`K` and type "align" or "distribute":

- **Align left edges**, **Align horizontal centres**, **Align right edges**, **Align top edges**, **Align vertical centres**, **Align bottom edges**.
- **Distribute horizontally**, **Distribute vertically**: equal gaps between three or more objects.
- **Flip horizontally**, **Flip vertically** mirror the selection.

## Landmarks

The stage, the mixing desk, a pillar, the exits: draw them with **Rectangle** (`U`), **Ellipse**, **Polygon** (click each corner, then the first again) or **Line**, and set their **Fill** and **Stroke** colours in the inspector. Buyers see them on the map.

Press `Escape` until nothing is selected before you draw one. While you are working inside a section (after double-clicking it), what you draw on it goes into that section; the **Layers** list shows where it went.

> **Not yet** The **Text** tool does nothing yet, so the stage cannot be named on the chart.

## Layers, locking and hiding

The **Layers** list shows everything on the chart, front to back. Right-click a line for:

- **Bring forward**, **Send backward**, **Bring to front**, **Send to back**: what is drawn over what.
- **Move up**, **Move down** (`Alt`+`↑`, `Alt`+`↓`): the same, by the keyboard.
- **Lock** (`Ctrl`+`L`): a locked object cannot be selected on the canvas or moved, so you cannot move it by accident while you work around it. Lock the floor plan picture.
- **Hide** (`Ctrl`+`H`): hidden objects disappear from the canvas. They stay in your draft, but they are also left out of the preview and of what you publish: a hidden block's seats are not on sale. Show them again before you publish if they should be sold.
- **Duplicate**, **Delete**.

A lock or an eye in a line's right-hand end means it is locked or hidden: choose it (**Unlock**, **Show**) to undo that.

## Rulers and guides

The rulers along the canvas measure in chart units. `Ctrl`+`R` shows or hides them.

A guide is a straight line across the canvas that objects snap to, to keep blocks level. Drag one out of the top ruler (a vertical guide) or the left ruler (a horizontal one) and drop it on the canvas. Drag it to move it; drag it back off the canvas to remove it. Without a mouse: press `Tab` until a ruler has focus, then `Enter` adds a guide at the middle of the view, the arrow keys move it, and `Delete` removes it.

> **Note** Guides are kept in your browser, not with the chart: a colleague does not see yours, and you do not see them on another computer.

## Snapping

While you drag, objects snap to other objects' edges and centres, to guides and to the grid, and a line shows what they snapped to. **Snap** in the status bar turns it off and on (`Ctrl`+`'`); hold `Alt` while dragging to stop it for that drag. **Grid** shows or hides the grid (`Ctrl`+`;`).
