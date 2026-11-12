# Start a chart

At the end of this page you will have a chart of your own open in the designer, with a picture of your floor plan under it to trace over.

You need to be signed in to the console as an owner, an admin or an editor of your workspace. A viewer can look at charts but not change them.

## Make a new chart

1. In the console, open **Charts** and choose **New chart**.
2. Type the chart's **Name**, as your staff call the room: "Riverside Hall", "Main auditorium". The **Key** fills in by itself. It is how your ticketing system names the chart and cannot be changed later, so keep it unless your developers asked for another.
3. **Start from** says what the new chart starts with. Keep **An empty chart** to draw your venue yourself, as this guide does. The other choices are the three templates ([Starting from a template or a file](#starting-from-a-template-or-a-file)) and **A reference image**, a floor plan to trace ([Trace a floor plan](#trace-a-floor-plan)).

   ![The New chart form, with the name Riverside Hall, the key riverside-hall, and Start from set to An empty chart](../images/designer-new-chart.webp)

4. Choose **Create chart**. The chart appears in the list.
5. Choose the chart's name, then **Open in designer**. The designer opens on an empty canvas, with **Start your chart** over it, and a short tour starts.

## The tour

The tour has five steps, each pointing at the part of the screen it talks about: draw a block, curve it, number it, assign a category, publish. Each step moves on by itself once you have done what it says, or choose **Next**. **Skip tour** closes it.

![The tour's first step, Draw a block, beside the tools](../images/designer-tour.webp)

The tour shows once in each browser. To see it again, press `Ctrl`+`K` (`⌘`+`K` on a Mac), type "tour" and choose **Show the designer tour**.

## Start your chart

Over an empty chart, **Start your chart** offers the ways to begin: **Draw from scratch**, **Place a reference image**, and, under **Start from a template**, **Theatre**, **Arena bowl** and **Banquet**. It is not in the way: the tools and the tour still work around it, and it goes away by itself as soon as you choose a drawing tool or draw something. **Draw from scratch**, or the cross (**Close the starting points**), closes it.

![Start your chart over an empty chart: draw from scratch, place a reference image, or start from a template](../images/designer-start.webp)

To see the starting points again later, open the **Menu** at the top left and choose **Starting points…**.

## Trace a floor plan

Most venues already have a floor plan: an architect's drawing, a fire plan, a scan of the old seating plan. Put it under your chart and draw the seats on top of it, so the chart matches the room.

The plan needs to be a picture file: PNG, JPEG or WebP. If you have a PDF, open it and take a screenshot of the page, or export it as a picture. A clear, straight scan is enough.

1. Choose the **Image tool** at the bottom of the tools on the left (or press `I`), or **Place a reference image** in **Start your chart**, and pick the picture file. If you chose **A reference image** for **Start from**, the designer opens on the new chart with **Start your chart**: choose **Place a reference image** there.
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
2. If the plan's seats are further apart than yours, the picture is too large: make it smaller. If they are closer together, make it larger. Select the picture in the **Layers** list, untick **Locked**, and change both **Width** and **Height** by the same proportion, so that the plan keeps its shape: to make it a fifth smaller, multiply both by 0.8. Each field changes only its own side.
3. Look again, repeat until the rows match, and lock the picture.

You can also leave the picture as it is and change the block's **Seat pitch** and **Row pitch** to match it instead. Either way, what matters is that the seats on your chart line up with the room.

## Starting from a template or a file

### From a template

The designer has three ready-made charts: a theatre (561 seats in curved stalls around a stage), an arena bowl (1,680 seats in 40 sections around a floor) and a banquet room (156 seats at long tables and a head table). A template is an ordinary chart: rename its sections, add or remove rows, change its categories, as on any chart you draw.

- **For a new chart:** in **New chart**, choose the template for **Start from**, such as **Theatre template (561 seats)**, then **Create chart**. The chart is made with the template's seats; open it in the designer as above.
- **For a chart already made:** while it is empty, choose the template in **Start your chart**. Otherwise open the **Menu**, choose **Starting points…** and choose the template: the designer asks first, since everything on the chart is replaced by the template. Choose **Replace**. One undo (`Ctrl`+`Z`) brings back what was there.

### From a chart file

Choose **Import chart file** on **Charts** and pick a chart file, then give the chart a **Name** and choose **Create chart**. A chart file is what **Export chart file** saves, in the designer's **Menu** or a chart's menu on **Charts**: use it to bring a chart from another Cavea workspace or server. To put a chart file's contents into a chart you have already made, open it in the designer and choose **Open chart file…** at the bottom of **Start your chart** (or in the **Menu**); on a chart that is not empty, it asks first, as a template does. Charts from other seating systems cannot be imported.
