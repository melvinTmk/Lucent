// Overview overlay rendering (pure DOM): one hit area per live DWM thumbnail (Rust draws
// the thumbnails over the window), with the hover ring around it and the app icon + window
// title under it.
import { t } from "../shared/i18n";
import type { OverviewShow } from "../shared/types";

export interface Handlers {
  pick: (hwnd: number) => void;
  /** Show "no open window" when this monitor has no tile (only when no monitor has any). */
  emptyHint?: boolean;
}

export function renderOverview(root: HTMLElement, show: OverviewShow, h: Handlers): void {
  const scale = show.scale && show.scale > 0 ? show.scale : 1;
  const tiles = show.tiles.map((tile, i) => {
    const el = document.createElement("div");
    el.className = "tile";
    el.setAttribute("role", "button");
    const title = tile.title.trim() || tile.appName || t.overview.untitled;
    el.setAttribute("aria-label", tile.appName && tile.appName !== title ? `${tile.appName} — ${title}` : title);
    el.style.left = `${tile.rect.x / scale}px`;
    el.style.top = `${tile.rect.y / scale}px`;
    el.style.width = `${tile.rect.w / scale}px`;
    el.style.height = `${tile.rect.h / scale}px`;
    el.style.setProperty("--i", String(i));
    const label = document.createElement("div");
    label.className = "label";
    if (tile.iconUrl) {
      const img = document.createElement("img");
      img.alt = "";
      img.draggable = false;
      img.src = tile.iconUrl;
      label.append(img);
    }
    const text = document.createElement("span");
    text.className = "title";
    text.textContent = title;
    label.append(text);
    el.append(label);
    el.addEventListener("click", (e) => {
      e.stopPropagation();
      h.pick(tile.hwnd);
    });
    return el;
  });
  if (tiles.length === 0 && h.emptyHint) {
    const empty = document.createElement("div");
    empty.className = "empty";
    empty.textContent = t.overview.empty;
    root.replaceChildren(empty);
    return;
  }
  root.replaceChildren(...tiles);
}
