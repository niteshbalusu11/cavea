# Draw seats

Seats are drawn in blocks: a block is a set of rows, each with the same number of seats, spaced evenly. At the end of this page you will have drawn a block over your plan's rows, curved it, and spaced it to match the room. You will also know how to make a block by typing its numbers instead of drawing.

## Draw a block of rows

1. Choose **Row block** in the tools (or press `R`).
2. Press on the canvas where the block's first corner goes, and drag. Rows and seats appear as you drag, and the status bar counts them: "8 rows × 14 seats".
3. To get an exact size, type it while you are still holding the button: `8x14` for 8 rows of 14 seats. The status bar shows what you typed. Press `Enter`, then let go.

   ![The status bar while drawing, showing the typed size 8x14](../images/designer-typing-size.webp)

4. The block appears, selected, and the **Inspector** shows its fields. The status bar now counts its seats: **112 seats**.

The block's top row is its front row, whichever way you dragged, and row letters start there. If your stage is at the top of the plan, that is the row nearest the stage. If it is elsewhere, turn the block afterwards with **Rotation** so its front row faces the stage.

The other ways to draw seats:

- **Arc block** (`A`): rows curved around a centre point, as in a theatre's circle or an arena. Click the centre, drag out to the first row and round to set how far it goes, then drag further out to add rows.
- **Single row** (`Shift`+`R`): one row on its own. Click where it starts and where it ends; move the pointer to bend it and click a third time, or press `Enter` to keep it straight. Use it for a row that does not belong to a block, such as a front row of wheelchair spaces.

## Put the block in place

Drag the block to move it, or type its position in the inspector: **X** and **Y** are where the middle of its front row sits, in chart units. **Rotation** turns it, in degrees.

## Curve it

Most theatres curve their rows towards the stage. With the block selected, a small diamond sits just behind the middle of its front row: the curve handle.

1. Drag the diamond back, away from the stage, to curve the rows around it. The status bar shows the curve's depth as you drag.
2. Let go when the rows follow the plan's curve.

To be exact, use the inspector: **Curve** is **Straight**, **Arc** or **Bulge**. **Arc** bends every row around one centre, at the **Radius** you type; **Bulge** pushes the middle of each row out by an **Amount**.

![A curved block of eight rows drawn over the floor plan](../images/designer-block.webp)

## Space the seats

In the inspector, under **Row block**:

| Field | What it changes |
|---|---|
| **Rows** | How many rows the block has |
| **Seats per row** | How many seats each row has |
| **Seat pitch** | The distance from one seat's centre to the next, along the row |
| **Row pitch** | The distance from one row to the next |
| **Alignment** | Where shorter rows sit: **Left**, **Centre** or **Right** |
| **Stagger** | Moves every other row sideways, so each seat looks between the two in front |
| **Skew** | Slants the block, for rows that step sideways as they go back |

Type a value and press `Enter` (or `Tab` to the next field). Each change can be undone on its own.

## Change single rows and seats

A block is one object. To work on one row or one seat in it, step in:

1. Double-click the block: you are now working on its rows. The inspector says **Rows of a row block**. (If the block is in a section, the first double-click selects the block inside the section; double-click again.)
2. Double-click a row: you are now working on its seats. The inspector says **Seats**.
3. Click a seat to select it, and press `Delete` to remove it, for a pillar or a gap. Its neighbours stay where they are.
4. Press `Escape` to step back out, once for each level.

## Type a block instead of drawing it

If you prefer typing numbers, or cannot use a mouse, the **Insert row block** form makes the same block:

1. Open the **Menu** at the top left and choose **Insert row block…** (or find it with `Ctrl`+`K`).
2. Choose **Straight** or **Arc** and fill in **Rows**, **Seats per row**, **Seat pitch** and **Row pitch**. For an arc, also the **Radius**.
3. **X** and **Y** are where the middle of the front row goes; they start at the middle of what you are looking at. **Rotation** turns the block.
4. **Insert into** puts the block in a section, if your chart has sections. **Category** is the price level its seats start with.
5. Choose **Insert**.

![The Insert row block form](../images/designer-insert-form.webp)

> **Warning** After **Insert**, the designer may stop showing your changes until you reload the page: the block is saved, but nothing else you do appears. Reload the page (`F5`) after inserting a block, and carry on. The total at the bottom of the form ("200 seats") also stays at the count the form opened with; it does not follow what you type.
