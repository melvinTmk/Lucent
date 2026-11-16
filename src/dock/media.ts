// Media island: artwork, title/artist and previous / play-pause / next for the current
// system media session (`media://state`). Nodes are reused across renders; the artwork
// `src` is only set when it changes, so it never reloads on a play/pause update.
import { invoke } from "../shared/ipc";
import { t } from "../shared/i18n";
import type { MediaAction, MediaState } from "../shared/types";

const svg = (body: string) =>
  `<svg viewBox="0 0 24 24" width="100%" height="100%" aria-hidden="true" fill="currentColor">${body}</svg>`;

const GLYPH = {
  previous: svg('<path d="M11.5 6.6v10.8a.6.6 0 0 1-.93.5L3.4 12.5a.6.6 0 0 1 0-1l7.17-5.4a.6.6 0 0 1 .93.5zM20.5 6.6v10.8a.6.6 0 0 1-.93.5l-7.17-5.4a.6.6 0 0 1 0-1l7.17-5.4a.6.6 0 0 1 .93.5z"/>'),
  next: svg('<path d="M3.5 6.6v10.8a.6.6 0 0 0 .93.5l7.17-5.4a.6.6 0 0 0 0-1L4.43 6.1a.6.6 0 0 0-.93.5zM12.5 6.6v10.8a.6.6 0 0 0 .93.5l7.17-5.4a.6.6 0 0 0 0-1L13.43 6.1a.6.6 0 0 0-.93.5z"/>'),
  play: svg('<path d="M7.5 5.2v13.6a.8.8 0 0 0 1.2.7l11-6.8a.8.8 0 0 0 0-1.4l-11-6.8a.8.8 0 0 0-1.2.7z"/>'),
  pause: svg('<rect x="6" y="5" width="4.2" height="14" rx="1.2"/><rect x="13.8" y="5" width="4.2" height="14" rx="1.2"/>'),
} as const;

const NOTE_SVG =
  '<svg viewBox="0 0 24 24" width="55%" height="55%" aria-hidden="true" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M9 18V5.5l11-2V16"/><circle cx="6.5" cy="18" r="2.5" fill="currentColor"/><circle cx="17.5" cy="16" r="2.5" fill="currentColor"/></svg>';

const wired = new WeakSet<HTMLElement>();

/** Click delegation: one listener per island, on the `[data-media-action]` buttons. */
function wire(island: HTMLElement): void {
  if (wired.has(island)) return;
  wired.add(island);
  island.addEventListener("click", (e) => {
    const btn = (e.target as Element).closest<HTMLButtonElement>("[data-media-action]");
    if (!btn || btn.disabled || !island.contains(btn)) return;
    void invoke("media_control", { action: btn.dataset.mediaAction as MediaAction });
  });
}

function part<K extends keyof HTMLElementTagNameMap>(
  parent: Element,
  cls: string,
  tag: K,
  init?: (el: HTMLElementTagNameMap[K]) => void,
): HTMLElementTagNameMap[K] {
  const found = parent.querySelector<HTMLElementTagNameMap[K]>(`:scope > .${cls}`);
  if (found) return found;
  const el = document.createElement(tag);
  el.className = cls;
  init?.(el);
  parent.appendChild(el);
  return el;
}

function setText(el: HTMLElement, text: string): void {
  if (el.textContent !== text) el.textContent = text;
  if (el.getAttribute("title") !== text) el.setAttribute("title", text);
  el.hidden = text === "";
}

function control(parent: Element, action: MediaAction, glyph: keyof typeof GLYPH, label: string, enabled: boolean): void {
  let b = parent.querySelector<HTMLButtonElement>(`[data-media-action="${action}"]`);
  if (!b) {
    b = document.createElement("button");
    b.type = "button";
    b.className = "media-btn";
    b.tabIndex = -1;
    b.dataset.mediaAction = action;
    parent.appendChild(b);
  }
  if (b.dataset.glyph !== glyph) {
    b.dataset.glyph = glyph;
    b.innerHTML = GLYPH[glyph];
  }
  b.setAttribute("aria-label", label);
  b.disabled = !enabled;
}

export function renderMedia(island: HTMLElement, m: MediaState): void {
  wire(island);

  const art = part(island, "media-art", "div");
  part(art, "media-placeholder", "span", (el) => (el.innerHTML = NOTE_SVG));
  let img = art.querySelector("img");
  if (m.artUrl) {
    if (!img) {
      img = document.createElement("img");
      img.alt = "";
      img.draggable = false;
      img.decoding = "async";
      const el = img;
      // A url the webview cannot decode: show the ♪ placeholder until the next artwork.
      img.addEventListener("error", () => {
        el.dataset.broken = "1";
        el.hidden = true;
        art.classList.remove("has-art");
      });
      art.appendChild(img);
    }
    if (img.getAttribute("src") !== m.artUrl) {
      delete img.dataset.broken;
      img.setAttribute("src", m.artUrl);
    }
    const ok = img.dataset.broken !== "1";
    img.hidden = !ok;
    art.classList.toggle("has-art", ok);
  } else {
    if (img) {
      img.hidden = true;
      img.removeAttribute("src");
      delete img.dataset.broken;
    }
    art.classList.remove("has-art");
  }

  const text = part(island, "media-text", "div");
  setText(part(text, "media-title", "span"), m.title);
  setText(part(text, "media-artist", "span"), m.artist);

  const controls = part(island, "media-controls", "div");
  control(controls, "previous", "previous", t.media.prev, m.canPrev);
  control(controls, "play_pause", m.playing ? "pause" : "play", m.playing ? t.media.pause : t.media.play, m.canPlayPause);
  control(controls, "next", "next", t.media.next, m.canNext);
}
