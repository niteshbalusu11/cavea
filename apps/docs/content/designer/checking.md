# Check the chart

The designer checks the whole chart while you work and lists what is wrong in the **Validation** panel. At the end of this page your chart will have no errors, and you will know which warnings matter for your room.

There are two kinds of finding:

- **Errors** stop the chart being published: **Publish** stays greyed out, and says "Fix the errors in the Validation panel to publish."
- **Warnings** do not stop it, but each is something buyers may notice. Fix them where you can.

## The validation panel

1. Open the **Validation** tab in the panels. The tab shows how many findings there are; the status bar shows "2 errors · 3 warnings", or **No problems**.
2. While you are working, the panel may say **Checking the whole chart…**: the full check runs a moment after you stop.
3. Each finding has a title, its code (such as E001) and a line of detail. Click it: the designer selects the object it is about and shows it on the canvas.
4. Fix it (below). The finding goes when the next check runs.

![The validation panel listing Duplicate label errors after a box was copied](../images/designer-validation.webp)

The line of detail is written by the part of Cavea that builds the chart, and it is terse. A number in it, such as "node 123", is the designer's own number for an object, the one in its name in the **Layers** list until you give it a name of your own ("Row block 123"). "row 57" is an internal number for a row, which the screen does not show: click the finding to be taken to it.

## Errors

### Duplicate label

Code E001. Two seats have the same full label, so a ticket could not say which one it is. The detail names the label and the other object that has it.

Causes: a copied block, which keeps its original's labels; two blocks in sections with the same label; two blocks without sections numbered the same way.

Fix: give the copy's section its own **Label**, or renumber the copy ([Number rows and seats](numbering.md)), so that every section, row and seat combination appears once.

### Seat without a category

Code E002. Some seats have no category, so they cannot be priced.

Cause: the block's **Category** is **Inherit**, and its section has none, or it is in no section.

Fix: choose a **Category** for the block, or for its section ([Categories](categories.md)).

### Empty label

Code E003. A row or a seat has no label: the detail says which row, and which seat in it.

Causes: a **Custom list** with fewer labels than the block has rows or seats; a list with an empty place in it ("A, , C").

Fix: open **Row and seat labels…** for the block and make the list long enough, or choose another **Scheme**.

### Area without a capacity

Code E004. A standing area has no number of places. The designer cannot draw areas yet, so you will not see this.

### Invalid geometry

Code E005. Something cannot be drawn: a shape or section outline that crosses itself or is flat, a size of zero, a colour that is not a colour, a number out of its range.

Fix: select it from the finding. Redraw a crossed outline; type a sensible value into the field the detail names. If the chart came from a file, the file may be damaged.

### Limit exceeded

Code E006. The chart is bigger than Cavea allows: too many seats, too many seats in a row, too many objects, or a label or text that is too long. The detail names the limit.

Fix: split a row that is too long into two blocks; shorten long labels. A chart over the seat limit is too large for one map: draw separate charts for separate halls.

### Unknown reference

Code E007. Something names what is not on the chart: a category that was deleted, or a picture that never finished uploading.

Fix: for a category, choose an existing one for the object. For a picture, select it: the inspector says it is missing; place the picture again with the **Image tool**, or delete it.

### Duplicate category key

Code E008. Two categories have the same key. The designer does not let you type a key that is taken, so this comes only from an imported file. Fix: give one of them another **Key** in the **Categories** tab.

### Duplicate id

Code E009. Two objects share the designer's internal number. This comes only from a damaged or hand-edited chart file. Fix: import the file again from its source, or delete one of the two objects and draw it again.

### Unsupported schema

Code E010. The chart file was made by a newer version of Cavea than this server runs. Fix: ask whoever runs your Cavea server to upgrade it, then import the file again.

### Not supported yet

Code "unsupported". The chart contains something this version of Cavea cannot sell yet: a table, a booth, a standing area, or a section inside a section. It comes from an imported file. Fix: delete it, or redraw it as rows of seats.

## Warnings

### Overlapping seats

Code W101. Seats are so close that they overlap: buyers could not tap one without the other.

Causes: two blocks drawn over each other; a **Seat pitch** or **Row pitch** smaller than the seats; a tight curve that squeezes the back rows.

Fix: move the blocks apart, make the pitch larger, or the curve gentler.

### No focal point

Code W102. The chart does not say where the stage is. The focal point is the point the seats face; Cavea uses it to suggest the best available seats (nearest the stage first) and to read left and right in the numbering panel.

A chart started in the console cannot have a focal point yet: this warning stays, and the best available seats are picked row by row from the front instead. A chart started from a template has one ([Starting from a template](start.md#starting-from-a-template-or-a-file)): there, choose **Focal point** (`F`) and click the middle of the stage to move it.

### Single-seat row

Code W103. A row has only one seat. That is fine for a box seat or a wheelchair space on its own; otherwise it usually means a block was drawn too narrow. Fix: make **Seats per row** larger, or delete the row.

### Seat outside its section

Code W104. A seat lies outside the outline you drew for its section, so on the buyer's map it looks as if it belongs to another part of the house. Fix: select the section and drag its outline's points to take the seat in, or move the seat's block back inside.

### Unused category

Code W105. A category has no seats. Buyers would see it in the map's key with nothing to choose. Fix: paint seats into it, or delete it with **Delete…** in its menu.

### No accessible seats

Code W106. No seat is marked accessible, so buyers who need a wheelchair space or step-free seat cannot find one on the map. Fix: make a category with **Accessible** ticked and put those seats in it ([Categories](categories.md)). If your venue truly has none, you can publish with this warning.

### Colours hard to tell apart

Code W107. Two categories' colours look alike, or a colour hardly shows against the map's light or dark background. Buyers would not know which price level a seat is. The **Categories** tab marks the category too. Fix: choose a clearly different colour ([Colours](categories.md#colours)).

### Text too small

Code W108. A piece of text on the chart is too small to read when the whole chart is on screen. Fix: make its **Size** larger. (Text cannot be added yet, so this comes only from imported files.)

### Section without a label

Code W109. A section has no **Label**, so its seats' labels have no section part and buyers cannot tell the section's name. Fix: select the section and type its **Label**.

### Chart very large

Code W110. The chart is drawn across a very large distance, more than 200,000 chart units, and seats far from the middle may be placed slightly off. Usually a block or picture was moved far away by accident. Fix: press `Ctrl`+`0` to see everything, find the stray object, and bring it back or delete it.
