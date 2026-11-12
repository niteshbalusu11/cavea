# Start a chart

At the end of this page you will have a chart of your own open in the designer, with a picture of your floor plan under it to trace over.

You need to be signed in to the console as an owner, an admin or an editor of your workspace. A viewer can look at charts but not change them.

## Make a new chart

1. In the console, open **Charts** and choose **New chart**.
2. Type the chart's **Name**, as your staff call the room: "Riverside Hall", "Main auditorium". The **Key** fills in by itself. It is how your ticketing system names the chart and cannot be changed later, so keep it unless your developers asked for another.

   ![The New chart form, with the name Riverside Hall and the key riverside-hall](../images/designer-new-chart.webp)

3. Choose **Create chart**. The chart appears in the list.
4. Choose the chart's name, then **Open in designer**. The designer opens on an empty canvas, and a short tour starts.

## The tour

The tour has five steps, each pointing at the part of the screen it talks about: draw a block, curve it, number it, assign a category, publish. Each step moves on by itself once you have done what it says, or choose **Next**. **Skip tour** closes it.

![The tour's first step, Draw a block, beside the tools](../images/designer-tour.webp)

The tour shows once in each browser. To see it again, press `Ctrl`+`K` (`⌘`+`K` on a Mac), type "tour" and choose **Show the designer tour**.

## Trace a floor plan

Most venues already have a floor plan: an architect's drawing, a fire plan, a scan of the old seating plan. Put it under your chart and draw the seats on top of it, so the chart matches the room.

The plan needs to be a picture file: PNG, JPEG or WebP. If you have a PDF, open it and take a screenshot of the page, or export it as a picture. A clear, straight scan is enough.

1. Choose the **Image tool** at the bottom of the tools on the left (or press `I`) and pick the picture file.
2. The picture appears on the canvas, selected, and the **Inspector** on the right shows it. While it uploads, the bar at the bottom says **Pictures uploading: 1**; wait until that goes.
3. Tick **Locked** in the inspector, so that clicking and dragging on the canvas while you draw never moves the picture.

![The designer with the floor plan placed and locked; the inspector shows its role, Reference](../images/designer-reference.webp)

### Reference or background

The inspector's **Role** says what the picture is for:

| Role | Who sees it | Use it for |
|---|---|---|
| **Reference** | Only you, in the designer. Buyers never see it. | A plan you trace over. This is what a new picture starts as. |
| **Background** | Buyers too, under the seats. | A picture you want buyers to see: the stage set, an illustrated view of the room. |

Keep a scanned plan as **Reference**: its lines and notes would only confuse buyers, and the chart draws the seats itself.

### Getting the size right

Distances in the designer are in chart units (**cu** beside a field). A new row block spaces its seats 55 units apart and its rows 95 apart, which suits a theatre seat. So that your rows sit on the plan's rows, change the picture's size until a row of seats drawn over it matches the plan:

1. Draw one row block over the front row of the plan ([Draw seats](seats.md)).
2. If the plan's seats are wider apart than yours, select the picture in the **Layers** list, untick **Locked**, and make **Width** larger; if they are closer, smaller. **Height** follows.
3. Lock it again.

You can also leave the picture as it is and change the block's **Seat pitch** and **Row pitch** to match it instead. Either way, what matters is that the seats on your chart line up with the room.

## Starting from a template or a file

The designer has three ready-made charts to start from: a theatre, an arena bowl and a banquet room. They are offered only in the designer's own window, not when you open a chart from the console. To use one:

1. In **Charts**, choose **Open the designer**. The designer opens with **New chart** and its starting points: **Draw from scratch**, **Place a reference image**, and under **Start from a template**, **Theatre**, **Arena bowl** and **Banquet**.

   ![The New chart screen: draw from scratch, place a reference image, or start from a template](../images/designer-start.webp)

2. Choose a template. It opens as a chart you can change.
3. Open the **Menu** at the top left and choose **Export chart file**. Your browser saves a file ending in `.json`.
4. Go back to **Charts** (the **Menu**'s **Back to console**), choose **Import chart file** and pick the file. Give the chart a **Name** and choose **Create chart**.

The same **Import chart file** opens a chart someone exported from another Cavea workspace. Charts from other seating systems cannot be imported.

> **Warning** A chart drawn after **Open the designer** is saved nowhere until you export it: close the window first and it is gone. Import it into **Charts** as soon as you have the start you want, and do the rest of the work there, where it is saved as you go ([Saving](saving.md)).
