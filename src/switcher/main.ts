// App switcher page. Rust (switcher.rs) builds the model, sizes and places the window,
// draws the window thumbnails and polls the keys; this page draws the glass content.
// After each `switcher://show` it acks with `switcher_ready` so Rust reveals the window
// only once the new apps are in the DOM.
import { invoke, on, onWindow } from "../shared/ipc";
import type { Settings } from "../shared/types";
import { renderSwitcher, renderWindows, selectApp, selectWindow, type Pick } from "./render";

const app = document.getElementById("app") as HTMLElement;
const island = document.createElement("div");
island.id = "island";
island.className = "island";
app.append(island);

const pick: Pick = (appId, hwnd) => void invoke("switcher_pick", hwnd === undefined ? { appId } : { appId, hwnd });

function applySettings(s: Settings | null): void {
  if (!s) return;
  document.body.toggleAttribute("data-reduce-motion", s.animations.reduceMotion);
  const mode = s.dock.glass;
  const dark = mode === "dark" || (mode === "auto" && window.matchMedia?.("(prefers-color-scheme: dark)").matches);
  document.documentElement.dataset.tone = dark ? "dark" : "light";
}

void invoke("get_settings").then(applySettings, () => {});
void on("settings://changed", applySettings);

void Promise.all([
  onWindow("switcher://show", (p) => {
    // Appears at once, like Cmd+Tab: the native glass under the page cannot fade with it.
    renderSwitcher(island, p, pick);
    void invoke("switcher_ready");
  }),
  onWindow("switcher://move", ({ index }) => selectApp(island, index)),
  onWindow("switcher://windows", (w) => renderWindows(island, w, pick)),
  onWindow("switcher://window-index", ({ index }) => selectWindow(island, index)),
  onWindow("switcher://hide", () => {
    island.classList.remove("expanded");
    island.replaceChildren();
  }),
]).then(() => invoke("switcher_ready"));
