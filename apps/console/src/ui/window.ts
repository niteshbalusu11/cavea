// The arithmetic of a virtual list with rows of one height: which rows a
// scroll position shows, with some rows beyond each edge.

export interface ListWindow {
  start: number;
  count: number;
}

export function windowFor(
  scrollTop: number,
  height: number,
  rowHeight: number,
  total: number,
  overscan: number,
): ListWindow {
  if (total <= 0 || rowHeight <= 0 || !(height > 0)) return { start: 0, count: 0 };
  const first = Math.floor(Math.max(0, scrollTop) / rowHeight);
  const start = Math.min(total - 1, Math.max(0, first - overscan));
  const end = Math.min(total, first + Math.ceil(height / rowHeight) + 1 + overscan);
  return { start, count: Math.max(0, end - start) };
}

/** The scroll position that shows row `index` whole, moving as little as
 * possible from `scrollTop`. */
export function scrollToShow(
  index: number,
  scrollTop: number,
  height: number,
  rowHeight: number,
): number {
  const top = index * rowHeight;
  if (top < scrollTop) return top;
  if (top + rowHeight > scrollTop + height) return top + rowHeight - height;
  return scrollTop;
}
