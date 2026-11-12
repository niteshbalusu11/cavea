# Categories

A category is a group of seats sold at one price level: "Stalls", "Circle", "Restricted view", "Wheelchair space". Each seat belongs to one. The chart says which seats are in which category and gives each a colour; the price is set for each event, by your box office or your ticketing system, not in the chart. At the end of this page your chart will have its categories and every seat will be in the right one.

Name categories in your venue's own words: buyers see the names in the map's key, beside the colours.

## Make your categories

A new chart has one category, **Standard**. Open the **Categories** tab in the panels at the bottom right.

1. Choose the arrow beside **Standard** (**Edit Standard**). Change its **Label** to your main price level, such as "Stalls", and press `Enter`.
2. Change its **Key** to match, such as `stalls`, and press `Enter`. The key is the name your ticketing system uses for the category; see [Keys](#keys) before you change it on a chart already in use.
3. Choose the arrow again to close it.
4. Choose **Add category** for each other level. Give each a **Label**, a **Key** and a **Colour** (type a colour such as `#B45309`, or choose one).
5. Tick **Accessible** on a category of wheelchair or accessible spaces. The buyer's map marks those seats, and buyers who need them can find them.

The list shows how many seats each category has ("110 objects"), and the word **Accessible**, with a wheelchair symbol, beside an accessible one. The menu at the end of each row (the three dots) has **Move up** and **Move down**, which set the order of the map's key, **Merge into…** and **Delete…**. Deleting a category that seats still use asks which category they move to.

## Colours

Buyers tell categories apart by colour, so the designer warns you, under the category, when two colours are too close:

![The categories panel warning that Box is hard to tell apart from Stalls](../images/designer-similar.webp)

- **Hard to tell apart from Stalls.**: the two colours look alike to many people, especially on a phone in sunlight or to someone with a colour vision deficiency. Choose a colour further away: a different hue, not a lighter or darker shade of the same one.
- "Weak against the canvas": the colour is too pale (or too dark) to see against the map's light or dark background. Make it stronger.

Both are warnings: you can publish with them, but buyers may struggle. They also appear in the **Validation** panel ([Colours hard to tell apart](checking.md#colours-hard-to-tell-apart)).

## Put seats in categories

A block's seats take the block's category. With the block selected, choose it in the inspector's **Category**. **Inherit** means the block takes its section's category ([Sections](arranging.md#sections)).

To put single seats in another category, paint them:

1. Step into the block's seats: double-click the block, then double-click the row, so that the inspector says **Seats** ([Change single rows and seats](seats.md#change-single-rows-and-seats)). The brush paints what a click would select, so without this it paints whole blocks.
2. In **Categories**, click the name of the category to paint with. Its row is marked **Active**, and the top of the list says "Active:" and its name: it is the category the brush paints and new blocks start with.
3. Choose **Category brush** in the tools (`C`).
4. Click seats, or drag across them. Each click is one step you can undo.
5. Choose **Select** (`V`) when you have finished.

![Two wheelchair spaces painted at the ends of the front row](../images/designer-brush.webp)

The count beside each category changes as you paint. A seat's own category wins over its block's, and a block's over its section's.

## Keys

Your ticketing system finds a seat's price by its category's key: `stalls` costs this much, `box` that much. So:

- Agree the keys with whoever sets up pricing, before the first event goes on sale.
- Once events use the chart, do not change a key: the next version you publish would have seats in a category your ticketing system does not know. Change the **Label** instead; buyers see only the label.
- Two categories cannot share a key.
