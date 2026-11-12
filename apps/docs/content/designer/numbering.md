# Number rows and seats

Every seat has a label, printed on the ticket and shown to the buyer: the section, the row and the seat, such as "Stalls-C-12". At the end of this page your rows and seats will be numbered the way the room's signs and your tickets already number them.

A new block is numbered A, B, C from the front row and 1, 2, 3 from the left. If that is right for your room, you have nothing to do.

## Open the numbering panel

1. Select one block, or several that should follow the same pattern (hold `Shift` and click each).
2. In the **Inspector**, choose **Row and seat labels…**. The inspector becomes the **Numbering** panel.
3. Change the fields (below). The canvas shows the new labels on the seats as you type, and **Preview** at the bottom of the panel lists the first rows. Nothing has changed yet.
4. Choose **Apply**. The labels change, as one step you can undo. **Reset** puts the fields back as they were.
5. Choose the arrow at the top (**Back to the inspector**) to leave the panel.

![The numbering panel: rows starting at J, seats odd-numbered, with the preview on the canvas and in the panel](../images/designer-numbering.webp)

## Row labels

| Field | What it does |
|---|---|
| **Scheme** | **A, B, C**; **1, 2, 3**; odd or even numbers only; **a, b, c**; **I, II, III**; or a **Custom list** you type, such as "AA, BB, A, B, C" |
| **Start at** | The first row's label: start at J to carry on after a block in front that ends at H |
| **Step** | Count by this much: 2 gives A, C, E |
| **Skip** | Letters never used, separated by commas: many venues skip I and O, which look like 1 and 0 |
| **Direction** | **Front to back** (the front row is A) or **Back to front** |
| **After Z** | What comes after Z: **AA, BB, CC** or **AA, AB, AC**. This one is for the whole chart |
| **Prefix**, **Suffix** | Text before or after every row's label |

## Seat labels

| Field | What it does |
|---|---|
| **Scheme** | **1, 2, 3**, **1, 3, 5 (odd)**, **2, 4, 6 (even)**, letters, or a **Custom list** |
| **Start at**, **Step** | As for rows: start at 15 to carry on from a block on the left that ends at 14 |
| **Direction** | **Left to right** or **Right to left** |
| **Continue from the previous row** | Seats are numbered on from one row to the next instead of starting again in each row, as some arenas do |
| **Prefix**, **Suffix** | Text before or after every seat's label |

Left and right are as a person sees them looking at the stage, from the point the chart calls its focal point. A chart started in the console has no focal point yet, and the panel says so ("has no focal point to face"): then **Left to right** means left to right on your screen, before the block is turned.

## Odd and even

Many theatres number the left side of the house with odd numbers and the right side with even numbers, both counting outwards from the centre aisle. Draw the two sides as two blocks, then:

1. Select the left block. Choose **1, 3, 5 (odd)** and **Right to left**, so 1 is beside the aisle. **Apply**.
2. Select the right block. Choose **2, 4, 6 (even)** and **Left to right**. **Apply**.
3. A centre block between them usually counts **1, 2, 3** **Left to right**, with a **Prefix** or **Suffix** if your tickets need to tell the blocks apart.

## Several blocks at once

With several blocks selected, a field that differs between them shows **Each keeps its own**. Change it, and **Set on all** shows it will be set on every block. To number seats on across blocks (seats 1 to 14 in the left block, 15 to 28 in the centre), tick **Continue across blocks**, and drag the blocks in **Numbering order** into the order the numbers run.

## Labels set by hand

A row or seat that was given a label of its own, outside the pattern, keeps it when you renumber, as long as **Keep labels set by hand** is ticked. Untick it to number everything by the pattern.

## The full label

The panel ends with **Full label of the first seat:** and that label, as the ticket shows it. It is made of the section's label, the row and the seat, joined by dashes. The pattern is the chart's **Seat label** setting: select nothing, and the inspector shows it under **Chart settings**, as `{section}-{row}-{seat}`.

> **Warning** A seat that is not in a section gets a label starting with a dash: "-C-12". Put every block in a section with a label ([Sections](arranging.md#sections)), or change **Seat label** to `{row}-{seat}` if your chart has no sections.
