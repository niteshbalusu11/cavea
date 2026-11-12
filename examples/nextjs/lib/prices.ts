// Your price list, in pence, by the category keys the chart's designer gave
// (these are the sample theatre's). The page shows these prices on the map;
// the backend charges from them, by the categories the Cavea server reports
// for the seats actually held.
export const PRICES: Record<string, number> = {
  premium: 6500,
  standard: 4500,
  value: 2500,
  access: 4500,
};

export const pounds = (pence: number): string =>
  new Intl.NumberFormat("en-GB", { style: "currency", currency: "GBP" }).format(pence / 100);
