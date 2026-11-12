// The documentation site's only script, an enhancement: every page reads
// and navigates without it. It adds the search box (an index built with the
// site, searched in the page: no service), copy buttons on code, and closes
// the menu on a narrow screen.
/** The words of a search query. */
const words = (q) =>
  q
    .toLowerCase()
    .split(/\s+/)
    .filter((w) => w.length > 0);

/** Where the first word of the query is, with some text around it. */
const snippet = (text, query) => {
  const at = text.toLowerCase().indexOf(words(query)[0] ?? "");
  const start = Math.max(0, at - 40);
  return `${start > 0 ? "…" : ""}${text.slice(start, start + 140)}${text.length > start + 140 ? "…" : ""}`;
};

(() => {
  const root = document.body.dataset.root ?? "";

  // The menu starts open, so that without this script it is still there;
  // on a phone it then closes, leaving the page's content first.
  const menu = document.querySelector("details.menu");
  const narrow = matchMedia("(max-width: 800px)");
  const fitMenu = () => {
    if (menu) menu.open = !narrow.matches;
  };
  fitMenu();
  narrow.addEventListener("change", fitMenu);

  // Copy buttons.
  for (const pre of document.querySelectorAll(".code pre")) {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "copy";
    button.textContent = "Copy";
    button.setAttribute("aria-label", "Copy the code");
    button.addEventListener("click", async () => {
      try {
        await navigator.clipboard.writeText(pre.textContent ?? "");
        button.textContent = "Copied";
      } catch {
        button.textContent = "Select and copy";
      }
      setTimeout(() => (button.textContent = "Copy"), 2000);
    });
    pre.parentElement?.append(button);
  }

  // Search.
  const form = document.querySelector("form.search");
  const input = document.getElementById("search-input");
  const results = document.getElementById("search-results");
  if (!form || !input || !results) return;
  form.hidden = false;
  /** @type {Array<{t: string, s: string, u: string, x: string, k: string}> | null} */
  let index = null;
  let loading = null;

  const load = () => {
    loading ??= fetch(`${root}search.json`)
      .then((r) => (r.ok ? r.json() : Promise.reject(new Error(String(r.status)))))
      .then((entries) => {
        index = entries.map((e) => ({ ...e, k: `${e.t} ${e.s} ${e.x}`.toLowerCase() }));
      })
      .catch(() => {
        loading = null;
      });
    return loading;
  };

  const rank = (query) => {
    const terms = words(query);
    if (terms.length === 0 || index === null) return [];
    const phrase = query.trim().toLowerCase();
    const scored = [];
    for (const e of index) {
      if (!terms.every((t) => e.k.includes(t))) continue;
      const s = e.s.toLowerCase();
      const t = e.t.toLowerCase();
      let score = 0;
      if (s === phrase) score += 60;
      else if (s.includes(phrase)) score += 30;
      if (t.includes(phrase)) score += 15;
      if (e.x.toLowerCase().includes(phrase)) score += 6;
      for (const term of terms) {
        if (s.includes(term)) score += 8;
        if (t.includes(term)) score += 4;
      }
      // A page's top section is its summary: keep it near its sections.
      if (e.s === "") score += 2;
      scored.push({ e, score });
    }
    return scored.toSorted((a, b) => b.score - a.score).slice(0, 12);
  };

  const show = () => {
    const query = input.value;
    results.replaceChildren();
    if (query.trim() === "") {
      results.hidden = true;
      return;
    }
    results.hidden = false;
    if (index === null) {
      const p = document.createElement("p");
      p.textContent = "Loading the search index…";
      results.append(p);
      void load().then(() => {
        if (input.value === query) show();
      });
      return;
    }
    const found = rank(query);
    if (found.length === 0) {
      const p = document.createElement("p");
      p.textContent = `Nothing matches “${query.trim()}”.`;
      results.append(p);
      return;
    }
    const list = document.createElement("ul");
    for (const { e } of found) {
      const a = document.createElement("a");
      a.href = `${root}${e.u}`;
      const title = document.createElement("strong");
      title.textContent = e.s === "" ? e.t : `${e.s}`;
      const where = document.createElement("small");
      where.textContent = e.s === "" ? snippet(e.x, query) : `${e.t} · ${snippet(e.x, query)}`;
      a.append(title, where);
      const li = document.createElement("li");
      li.append(a);
      list.append(li);
    }
    results.append(list);
  };

  input.addEventListener("focus", () => void load());
  input.addEventListener("input", show);
  form.addEventListener("submit", (event) => {
    event.preventDefault();
    const first = results.querySelector("a");
    if (first) location.href = first.href;
  });
  form.addEventListener("keydown", (event) => {
    const links = [...results.querySelectorAll("a")];
    const at = links.indexOf(document.activeElement);
    if (event.key === "ArrowDown" && links.length > 0) {
      event.preventDefault();
      links[Math.min(at + 1, links.length - 1)]?.focus();
    } else if (event.key === "ArrowUp" && at >= 0) {
      event.preventDefault();
      if (at === 0) input.focus();
      else links[at - 1]?.focus();
    } else if (event.key === "Escape") {
      input.value = "";
      show();
      input.focus();
    }
  });
  document.addEventListener("click", (event) => {
    if (!form.contains(event.target)) results.hidden = true;
  });
  document.addEventListener("keydown", (event) => {
    const typing = event.target instanceof HTMLElement && event.target.closest("input, textarea");
    if (event.key === "/" && !typing) {
      event.preventDefault();
      input.focus();
    }
  });
})();
