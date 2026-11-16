// Thumbnails popup page. Rust (thumbs.rs) sizes/places the window and registers the DWM
// thumbnails; this page draws one glass card per window around its rect, plus the title.
// Tone: the payload carries the tone of the dock the popup belongs to (Rust reads the
// dock's last `dock://tone`), applied as `data-tone` so it matches that dock.
import { invoke, onWindow } from "../shared/ipc";
import { t } from "../shared/i18n";
import type { ThumbsShow } from "../shared/types";

/** Card margin around the thumbnail (CSS px); smaller than Rust's 16 px gap. */
const PAD = 8;

const app = document.getElementById("app") as HTMLElement;
const popup = document.createElement("div");
popup.id = "popup";
popup.className = "island";
popup.setAttribute("role", "menu");
popup.setAttribute("aria-label", t.thumbs.label);
app.append(popup);

function show(p: ThumbsShow): void {
  document.documentElement.dataset.tone = p.tone;
  // Every row is as tall as its tallest thumbnail slot (Rust: thumb_h + title_h).
  const cellH = Math.max(...p.rects.map((r) => r.h)) + p.titleH;
  const cards = p.rects.map((r, i) => {
    const card = document.createElement("div");
    card.className = "card";
    card.setAttribute("role", "menuitem");
    const title = p.titles[i]?.trim() || t.thumbs.untitled;
    // No `title` attribute: its native tooltip would repeat the visible label below.
    card.setAttribute("aria-label", title);
    card.style.left = `${r.x / p.scale - PAD}px`;
    card.style.top = `${r.y / p.scale - PAD}px`;
    card.style.width = `${r.w / p.scale + 2 * PAD}px`;
    card.style.height = `${cellH / p.scale + PAD}px`;
    const label = document.createElement("div");
    label.className = "title";
    label.textContent = title;
    card.append(label);
    card.addEventListener("click", () => void invoke("thumbs_pick", { hwnd: p.hwnds[i] }));
    return card;
  });
  popup.replaceChildren(...cards);
}

// The docks' hover machines keep the popup open while the pointer is on it (hover.ts).
document.documentElement.addEventListener("pointerenter", () => void invoke("thumbs_pointer", { inside: true }));
document.documentElement.addEventListener("pointerleave", () => void invoke("thumbs_pointer", { inside: false }));

// Rust builds this window on the first request and waits for this signal before sending it.
void Promise.all([
  onWindow("thumbs://show", show),
  onWindow("thumbs://hide", () => popup.replaceChildren()),
]).then(() => invoke("thumbs_ready"));
