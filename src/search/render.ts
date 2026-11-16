// Search results list, rendered into a container from (query, layout, selected). Pure DOM:
// no IPC, no timers, so it is unit-tested with jsdom. Nodes are keyed (sections by group
// key, rows by entry id, the hero row reused) so typing never rebuilds unchanged rows:
// icons don't reload and the list doesn't flicker.
import type { SearchEntry, SearchGroup, SearchHit, ValoRow } from "../shared/types";
import { t } from "../shared/i18n";
import { classifyShape } from "../shared/iconShape";

export const GRID_COLS = 8;
/** Bottom padding of the list, kept visible when the last row is revealed. */
const LIST_PAD = 6;

export interface Section {
  key: SearchGroup["key"];
  /** Emojis are a 2D grid of glyphs; every other group is a list of rows. */
  grid: boolean;
  /** Index of the section's first item in `Layout.hits`. */
  start: number;
  items: SearchHit[];
}

export interface Layout {
  /** Calculator result, or the only result when it is a command: shown large on top. */
  hero: SearchHit | null;
  sections: Section[];
  /** Every hit in display (= keyboard) order: hero first, then the sections. */
  hits: SearchHit[];
}

export type Dir = "up" | "down" | "left" | "right";

// ---------- layout ----------

function isHero(h: SearchHit, total: number): boolean {
  return h.entry.kind === "command" && (h.entry.id === "cmd:calc" || total === 1);
}

export function layout(groups: SearchGroup[]): Layout {
  const total = groups.reduce((n, g) => n + g.items.length, 0);
  let hero: SearchHit | null = null;
  for (const g of groups) {
    if (g.key === "recent") continue;
    hero = g.items.find((h) => isHero(h, total)) ?? null;
    if (hero) break;
  }
  const hits: SearchHit[] = hero ? [hero] : [];
  const sections: Section[] = [];
  for (const g of groups) {
    const items = g.items.filter((h) => h !== hero);
    if (items.length === 0) continue;
    sections.push({ key: g.key, grid: g.key === "emoji", start: hits.length, items });
    hits.push(...items);
  }
  return { hero, sections, hits };
}

// ---------- keyboard ----------

interface Block {
  start: number;
  count: number;
  cols: number;
}

function blocks(lay: Layout): Block[] {
  const out: Block[] = lay.hero ? [{ start: 0, count: 1, cols: 1 }] : [];
  for (const s of lay.sections) out.push({ start: s.start, count: s.items.length, cols: s.grid ? GRID_COLS : 1 });
  return out;
}

/** Where ↑ lands when it enters `b` from below: its last row (first cell in a grid). */
function entryFromBelow(b: Block): number {
  return b.cols === 1 ? b.start + b.count - 1 : b.start + Math.floor((b.count - 1) / b.cols) * b.cols;
}

/** New selection after an arrow key, or null when the key is not for the list (←/→
 *  outside a grid: the input's caret keeps them). ↑/↓ wrap around the whole list; in a
 *  grid they move by row and leave the grid at its first/last row. */
export function moveSelection(lay: Layout, sel: number, dir: Dir): number | null {
  const bs = blocks(lay);
  if (bs.length === 0) return null;
  const bi = bs.findIndex((b) => sel >= b.start && sel < b.start + b.count);
  if (bi < 0) return 0;
  const b = bs[bi];
  const pos = sel - b.start;
  if (dir === "left" || dir === "right") {
    if (b.cols === 1) return null;
    return b.start + Math.min(b.count - 1, Math.max(0, pos + (dir === "right" ? 1 : -1)));
  }
  if (b.cols > 1) {
    const row = Math.floor(pos / b.cols);
    const lastRow = Math.floor((b.count - 1) / b.cols);
    if (dir === "down" && row < lastRow) return b.start + Math.min(pos + b.cols, b.count - 1);
    if (dir === "up" && row > 0) return sel - b.cols;
  } else {
    if (dir === "down" && pos < b.count - 1) return sel + 1;
    if (dir === "up" && pos > 0) return sel - 1;
  }
  if (dir === "down") return bs[(bi + 1) % bs.length].start;
  return entryFromBelow(bs[(bi - 1 + bs.length) % bs.length]);
}

// ---------- text ----------

const segmenter =
  typeof Intl !== "undefined" && "Segmenter" in Intl ? new Intl.Segmenter(undefined, { granularity: "grapheme" }) : null;

/** User-perceived characters: the unit of Rust's match indices (nucleo graphemes). */
export function splitGraphemes(s: string): string[] {
  return segmenter ? Array.from(segmenter.segment(s), (x) => x.segment) : Array.from(s);
}

/** `title` as text nodes, with the graphemes at `indices` wrapped in `<b>` (runs merged). */
export function highlight(title: string, indices: number[]): DocumentFragment {
  const frag = document.createDocumentFragment();
  if (indices.length === 0) {
    frag.append(title);
    return frag;
  }
  const marked = new Set(indices);
  let run = "";
  let bold = false;
  const flush = () => {
    if (!run) return;
    if (bold) {
      const b = document.createElement("b");
      b.textContent = run;
      frag.append(b);
    } else {
      frag.append(run);
    }
    run = "";
  };
  splitGraphemes(title).forEach((g, i) => {
    const m = marked.has(i);
    if (m !== bold) {
      flush();
      bold = m;
    }
    run += g;
  });
  flush();
  return frag;
}

/** Emoji entries: the glyph travels in `keywords[0]`, the title is "glyph label". */
function emojiParts(e: SearchEntry): { glyph: string; label: string } {
  const glyph = e.keywords[0] ?? splitGraphemes(e.title)[0] ?? "";
  const label = glyph && e.title.startsWith(glyph) ? e.title.slice(glyph.length).trim() : e.title;
  return { glyph, label };
}

// ---------- icons ----------

// 20×20 line glyphs for entries without an icon (stroke = currentColor). Constants only:
// no entry data ever reaches innerHTML.
const SVG: Record<string, string> = {
  app: '<rect x="4" y="4" width="12" height="12" rx="3.2"/>',
  folder: '<path d="M2.75 6.25c0-.83.67-1.5 1.5-1.5h3.4l1.6 1.75h6.5c.83 0 1.5.67 1.5 1.5v6.75c0 .83-.67 1.5-1.5 1.5H4.25c-.83 0-1.5-.67-1.5-1.5z"/>',
  file: '<path d="M5.75 2.75h5.5l3.5 3.5v10c0 .55-.45 1-1 1h-8c-.55 0-1-.45-1-1v-12.5c0-.55.45-1 1-1z"/><path d="M11.25 2.75v3.5h3.5"/>',
  clipboard: '<rect x="4.75" y="4" width="10.5" height="13.25" rx="1.75"/><path d="M7.75 2.75h4.5v2.5h-4.5z"/>',
  web: '<circle cx="10" cy="10" r="7"/><path d="M3 10h14M10 3c2 2.2 2.8 4.6 2.8 7s-.8 4.8-2.8 7c-2-2.2-2.8-4.6-2.8-7S8 5.2 10 3z"/>',
  quit: '<circle cx="10" cy="10" r="7"/><path d="M7.5 7.5l5 5M12.5 7.5l-5 5"/>',
  calc: '<rect x="4.5" y="2.75" width="11" height="14.5" rx="2"/><path d="M7 6.25h6M7.25 10h.01M10 10h.01M12.75 10h.01M7.25 13.25h.01M10 13.25h.01M12.75 13.25h.01"/>',
  command: '<path d="M4.5 6l4 4-4 4M10.5 14.5h5"/>',
  valorant: '<path d="M3 4.5l7.5 10.5h4L7 4.5zM17 4.5l-4.75 6.5"/>',
};

function glyphKind(e: SearchEntry): string {
  switch (e.kind) {
    case "app":
      return "app";
    case "project":
      return "folder";
    case "file":
      return "file";
    case "clipboard":
      return "clipboard";
    case "emoji":
      return "app";
    case "valorant":
      return "valorant";
    case "command":
      if (e.id === "cmd:calc") return "calc";
      if (e.id.startsWith("cmd:web")) return "web";
      if (e.id.startsWith("cmd:kill")) return "quit";
      return "command";
  }
}

function updateIcon(span: HTMLElement, e: SearchEntry): void {
  const emojiGlyph = e.kind === "emoji" ? emojiParts(e).glyph : "";
  const key = e.icon ? `img:${e.icon}` : emojiGlyph ? `emoji:${emojiGlyph}` : `svg:${glyphKind(e)}`;
  if (span.dataset.key === key) return;
  span.dataset.key = key;
  span.className = "icon";
  span.replaceChildren();
  if (e.icon) {
    const img = document.createElement("img");
    img.alt = "";
    img.draggable = false;
    img.decoding = "async";
    // Icons come from the `icon` scheme (another origin): CORS lets the canvas read them.
    img.crossOrigin = "anonymous";
    img.addEventListener("load", () => classifyShape(img, span));
    img.src = e.icon;
    span.append(img);
  } else if (emojiGlyph) {
    span.classList.add("glyph");
    span.textContent = emojiGlyph;
  } else {
    span.classList.add("tile");
    span.innerHTML = `<svg viewBox="0 0 20 20" aria-hidden="true">${SVG[glyphKind(e)]}</svg>`;
  }
}

// ---------- Valorant tiles ----------

function span(className: string, text: string): HTMLSpanElement {
  const n = el("span", className);
  n.textContent = text;
  return n;
}

function valoImg(className: string, src: string): HTMLImageElement {
  const img = el("img", className);
  img.alt = "";
  img.draggable = false;
  img.decoding = "async";
  img.addEventListener("error", () => img.remove());
  img.src = src;
  return img;
}

function valoGlyph(): HTMLElement {
  const g = el("span", "valo-glyph");
  g.innerHTML = `<svg viewBox="0 0 20 20" aria-hidden="true">${SVG.valorant}</svg>`;
  return g;
}

export function valoResult(won: boolean | null): { label: string; cls: string } {
  return won === true ? { label: "VICTOIRE", cls: "win" } : won === false ? { label: "DÉFAITE", cls: "loss" } : { label: "ÉGALITÉ", cls: "draw" };
}

function signedRr(n: number): string {
  return `${n > 0 ? "+" : n < 0 ? "−" : "±"}${Math.abs(n)} RR`;
}

/** Rank hero (tier emblem, name, RR bar) or a match tile (map strip, agent, result, score,
 *  K/D/A). Rebuilt only when its data changes (pictures arriving included). */
function updateValoRow(row: HTMLElement, e: SearchEntry, v: ValoRow): void {
  const sig = JSON.stringify(v);
  if (row.dataset.valo === sig) return;
  row.dataset.valo = sig;
  delete row.dataset.sig;
  row.setAttribute("aria-label", `${e.title} — ${e.subtitle}`);
  row.classList.remove("valo-rank", "valo-tile", "win", "loss", "draw");
  if (v.type === "rank") {
    row.classList.add("valo-rank");
    const emblem = el("div", "valo-emblem");
    emblem.append(v.tierIcon ? valoImg("valo-tier", v.tierIcon) : valoGlyph());
    const info = el("div", "valo-info");
    const rr = Math.max(0, Math.min(100, v.rr));
    const bar = el("div", "valo-bar");
    const fill = el("div", "valo-bar-fill");
    fill.style.width = `${rr}%`;
    bar.append(fill);
    bar.setAttribute("role", "presentation");
    const meta = el("div", "valo-meta");
    meta.append(span("valo-rr-now", `${v.rr} / 100 RR`));
    if (v.nextTier && v.rrToNext != null) meta.append(span("valo-rr-next", `${v.rrToNext} RR avant ${v.nextTier}`));
    info.append(span("valo-kicker", "Rang actuel"), span("valo-tier-name", v.tier), bar, meta);
    const delta = el("div", "valo-delta");
    delta.classList.add(v.lastChange > 0 ? "up" : v.lastChange < 0 ? "down" : "flat");
    delta.append(span("valo-delta-value", signedRr(v.lastChange)), span("valo-delta-label", "dernier match"));
    row.replaceChildren(emblem, info, delta);
    return;
  }
  const r = valoResult(v.won);
  row.classList.add("valo-tile", r.cls);
  const bg = el("div", "valo-strip");
  if (v.mapImage) bg.append(valoImg("valo-map", v.mapImage));
  const agent = el("div", "valo-agent");
  agent.append(v.agentIcon ? valoImg("valo-agent-img", v.agentIcon) : valoGlyph());
  const main = el("div", "valo-main");
  main.append(span("valo-result", r.label), span("valo-sub", [v.map, v.agent].filter(Boolean).join(" · ")));
  const score = span("valo-score", `${v.score[0]}\u2009–\u2009${v.score[1]}`);
  const kda = el("div", "valo-kda");
  kda.append(span("valo-kda-label", "K / D / A"), span("valo-kda-value", `${v.kda[0]} / ${v.kda[1]} / ${v.kda[2]}`));
  const extra = el("div", "valo-kda valo-hs");
  if (v.hsPct != null) extra.append(span("valo-kda-label", "HS"), span("valo-kda-value", `${v.hsPct} %`));
  else if (v.acs != null) extra.append(span("valo-kda-label", "ACS"), span("valo-kda-value", String(v.acs)));
  row.replaceChildren(bg, agent, main, score, kda, extra);
}

// ---------- nodes ----------

function el<K extends keyof HTMLElementTagNameMap>(tag: K, className: string): HTMLElementTagNameMap[K] {
  const n = document.createElement(tag);
  n.className = className;
  return n;
}

function setOption(n: HTMLElement, idx: number, selected: number): void {
  n.dataset.index = String(idx);
  n.id = `opt-${idx}`;
  const on = idx === selected;
  n.classList.toggle("selected", on);
  n.setAttribute("aria-selected", String(on));
}

function createRow(className: string): HTMLElement {
  const row = el("div", className);
  row.setAttribute("role", "option");
  const text = el("div", "text");
  text.append(el("div", "title"), el("div", "sub"));
  row.append(el("span", "icon"), text);
  return row;
}

function updateText(row: HTMLElement, hit: SearchHit, render: (title: HTMLElement) => void): void {
  const title = row.querySelector<HTMLElement>(".title")!;
  const sub = row.querySelector<HTMLElement>(".sub")!;
  const sig = `${hit.entry.id}\u0000${hit.entry.title}\u0000${hit.indices.join(",")}`;
  if (row.dataset.sig !== sig) {
    row.dataset.sig = sig;
    render(title);
  }
  if (sub.textContent !== hit.entry.subtitle) sub.textContent = hit.entry.subtitle;
}

function updateRow(row: HTMLElement, hit: SearchHit, idx: number, selected: number): void {
  row.dataset.id = hit.entry.id;
  setOption(row, idx, selected);
  if (hit.entry.valo) {
    updateValoRow(row, hit.entry, hit.entry.valo);
    return;
  }
  if (row.dataset.valo !== undefined) {
    // Was a Valorant tile (same id, now plain): back to the standard row.
    delete row.dataset.valo;
    delete row.dataset.sig;
    row.classList.remove("valo-rank", "valo-tile", "win", "loss", "draw");
    row.removeAttribute("aria-label");
    const text = el("div", "text");
    text.append(el("div", "title"), el("div", "sub"));
    row.replaceChildren(el("span", "icon"), text);
  }
  updateText(row, hit, (title) => title.replaceChildren(highlight(hit.entry.title, hit.indices)));
  updateIcon(row.querySelector<HTMLElement>(".icon")!, hit.entry);
}

function updateHero(row: HTMLElement, hit: SearchHit, selected: number): void {
  row.dataset.id = hit.entry.id;
  setOption(row, 0, selected);
  updateText(row, hit, (title) => {
    const m = /^=\s*(.*)$/s.exec(hit.entry.title);
    if (m && hit.entry.id === "cmd:calc") {
      // "= 42": the result is the hero, the "=" a quiet lead-in.
      const eq = el("span", "eq");
      eq.textContent = "=";
      title.replaceChildren(eq, m[1]);
    } else {
      title.replaceChildren(highlight(hit.entry.title, hit.indices));
    }
  });
  updateIcon(row.querySelector<HTMLElement>(".icon")!, hit.entry);
}

function updateCell(cell: HTMLElement, hit: SearchHit, idx: number, selected: number): void {
  cell.dataset.id = hit.entry.id;
  setOption(cell, idx, selected);
  const { glyph, label } = emojiParts(hit.entry);
  if (cell.textContent !== glyph) cell.textContent = glyph;
  cell.title = label;
  cell.setAttribute("aria-label", label);
}

/** Puts `order` as the first children of `parent`, moving only what is out of place. */
function arrange(parent: HTMLElement, order: HTMLElement[]): void {
  order.forEach((n, idx) => {
    if (parent.children[idx] !== n) parent.insertBefore(n, parent.children[idx] ?? null);
  });
}

function keyed(parent: HTMLElement, selector: string): Map<string, HTMLElement> {
  const m = new Map<string, HTMLElement>();
  for (const n of parent.querySelectorAll<HTMLElement>(`:scope > ${selector}`)) m.set(n.dataset.id ?? "", n);
  return m;
}

function createSection(key: Section["key"]): HTMLElement {
  const s = el("section", "group");
  s.dataset.key = key;
  s.setAttribute("role", "group");
  const header = el("div", "header");
  header.textContent = t.search.groups[key];
  header.id = `group-${key}`;
  s.setAttribute("aria-labelledby", header.id);
  s.append(header, el("div", "body"));
  return s;
}

function updateSection(s: HTMLElement, sec: Section, selected: number): void {
  const body = s.querySelector<HTMLElement>(":scope > .body")!;
  body.classList.toggle("grid", sec.grid);
  body.classList.toggle("rows", !sec.grid);
  const itemClass = sec.grid ? "cell" : "row";
  for (const stale of body.querySelectorAll(`:scope > :not(.${itemClass})`)) stale.remove();
  const existing = keyed(body, `.${itemClass}`);
  const order = sec.items.map((hit, i) => {
    const id = hit.entry.id;
    let n = existing.get(id);
    existing.delete(id);
    if (!n) {
      n = sec.grid ? el("div", "cell") : createRow("row");
      if (sec.grid) n.setAttribute("role", "option");
    }
    if (sec.grid) updateCell(n, hit, sec.start + i, selected);
    else updateRow(n, hit, sec.start + i, selected);
    return n;
  });
  for (const n of existing.values()) n.remove();
  arrange(body, order);

  // Grid caption: the name of the selected emoji (or of the first one).
  let caption = s.querySelector<HTMLElement>(":scope > .caption");
  if (sec.grid) {
    if (!caption) {
      caption = el("div", "caption");
      caption.setAttribute("aria-hidden", "true");
      s.append(caption);
    }
    const inside = selected >= sec.start && selected < sec.start + sec.items.length;
    const label = emojiParts(sec.items[inside ? selected - sec.start : 0].entry).label;
    if (caption.textContent !== label) caption.textContent = label;
  } else {
    caption?.remove();
  }
}

export function renderResults(root: HTMLElement, query: string, lay: Layout, selected: number): void {
  const order: HTMLElement[] = [];

  let hero = root.querySelector<HTMLElement>(":scope > .hero");
  if (lay.hero) {
    hero ??= createRow("hero");
    updateHero(hero, lay.hero, selected);
    order.push(hero);
  } else {
    hero?.remove();
  }

  const sections = new Map<string, HTMLElement>();
  for (const s of root.querySelectorAll<HTMLElement>(":scope > .group")) sections.set(s.dataset.key ?? "", s);
  for (const sec of lay.sections) {
    let s = sections.get(sec.key);
    sections.delete(sec.key);
    s ??= createSection(sec.key);
    updateSection(s, sec, selected);
    order.push(s);
  }
  for (const s of sections.values()) s.remove();

  let empty = root.querySelector<HTMLElement>(":scope > .empty");
  if (lay.hits.length === 0 && query.trim() !== "") {
    if (!empty) {
      empty = el("div", "empty");
      empty.textContent = t.search.empty;
    }
    order.push(empty);
  } else {
    empty?.remove();
  }

  arrange(root, order);
}

function topWithin(n: HTMLElement, root: HTMLElement): number {
  return n.getBoundingClientRect().top - root.getBoundingClientRect().top + root.scrollTop;
}

/** Scrolls `root` (the list) the least needed to show the selected option; the first row
 *  of a section also brings its header into view. No-op without layout (jsdom). */
export function revealSelected(root: HTMLElement, selected: number): void {
  const n = root.querySelector<HTMLElement>(`[data-index="${selected}"]`);
  if (!n) return;
  if (selected === 0) {
    root.scrollTop = 0;
    return;
  }
  let top = topWithin(n, root);
  const bottom = top + n.offsetHeight;
  const section = n.closest<HTMLElement>(".group");
  const first = section?.querySelector<HTMLElement>("[data-index]");
  if (section && first && topWithin(first, root) === top) top = topWithin(section, root);
  if (top < root.scrollTop) root.scrollTop = top;
  else if (bottom > root.scrollTop + root.clientHeight) root.scrollTop = bottom - root.clientHeight + LIST_PAD;
}
