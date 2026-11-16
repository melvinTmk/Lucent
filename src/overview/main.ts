// Overview overlay page (one per monitor). Rust (overview.rs) covers the monitor with this
// window and glides the live DWM thumbnails into place; this page fades the dimmed
// backdrop in, then draws the hover rings and the labels under the tiles.
import { invoke, on, onWindow } from "../shared/ipc";
import type { Settings } from "../shared/types";
import { renderOverview } from "./render";

const app = document.getElementById("app") as HTMLElement;
const backdrop = document.createElement("div");
backdrop.id = "backdrop";
const tiles = document.createElement("div");
tiles.id = "tiles";
app.append(backdrop, tiles);

const close = () => void invoke("overview_close");
// A click anywhere but on a tile (tiles stop the event) closes the overview.
app.addEventListener("click", close);
window.addEventListener("keydown", (e) => {
  if (e.key === "Escape") close();
});

function applySettings(s: Settings | null): void {
  if (s) document.body.toggleAttribute("data-reduce-motion", s.animations.reduceMotion);
}
void invoke("get_settings").then(applySettings, () => {});
void on("settings://changed", applySettings);

void Promise.all([
  onWindow("overview://show", (p) => {
    renderOverview(tiles, p, { pick: (hwnd) => void invoke("overview_pick", { hwnd }), emptyHint: p.emptyHint });
    document.body.classList.remove("open");
    requestAnimationFrame(() => document.body.classList.add("open"));
    void invoke("overview_ready");
  }),
  onWindow("overview://hide", () => {
    document.body.classList.remove("open");
    // Labels go at once; the backdrop fades while the thumbnails glide back.
    tiles.replaceChildren();
  }),
]).then(() => invoke("overview_ready"));
