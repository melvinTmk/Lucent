// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { OverviewShow } from "../shared/types";
import { renderOverview } from "./render";

const show: OverviewShow = {
  monitorId: "\\\\.\\DISPLAY1",
  scale: 2,
  tiles: [
    { hwnd: 11, title: "Inbox — Mail", appName: "Mail", iconUrl: "icon://mail", rect: { x: 100, y: 200, w: 800, h: 600 } },
    { hwnd: 12, title: "", appName: "Notes", iconUrl: "", rect: { x: 1000, y: 200, w: 400, h: 600 } },
  ],
};

let root: HTMLElement;
beforeEach(() => {
  document.body.innerHTML = '<div id="tiles"></div>';
  root = document.getElementById("tiles")!;
});

const tiles = () => [...root.querySelectorAll<HTMLElement>(".tile")];

describe("renderOverview", () => {
  it("places_tiles_in_css_px", () => {
    renderOverview(root, show, { pick: () => {} });
    const t = tiles()[0];
    expect([t.style.left, t.style.top, t.style.width, t.style.height]).toEqual(["50px", "100px", "400px", "300px"]);
  });

  it("labels_title_and_app_icon_under_each_tile", () => {
    renderOverview(root, show, { pick: () => {} });
    expect(tiles().map((t) => t.querySelector(".label .title")!.textContent)).toEqual(["Inbox — Mail", "Notes"]);
    expect(tiles()[0].querySelector<HTMLImageElement>(".label img")!.getAttribute("src")).toBe("icon://mail");
    expect(tiles()[1].querySelector(".label img"), "no icon url: no image").toBeNull();
    expect(tiles()[0].getAttribute("aria-label")).toBe("Mail — Inbox — Mail");
    expect(tiles()[1].getAttribute("aria-label")).toBe("Notes");
  });

  it("click_picks_the_window_and_does_not_reach_the_backdrop", () => {
    const pick = vi.fn();
    const backdrop = vi.fn();
    root.addEventListener("click", backdrop);
    renderOverview(root, show, { pick });
    tiles()[1].click();
    expect(pick).toHaveBeenCalledWith(12);
    expect(backdrop).not.toHaveBeenCalled();
  });

  it("empty_monitor_shows_a_hint_only_when_asked", () => {
    renderOverview(root, { ...show, tiles: [] }, { pick: () => {}, emptyHint: true });
    expect(root.querySelector(".empty")!.textContent).toBe("Aucune fenêtre ouverte");
    renderOverview(root, { ...show, tiles: [] }, { pick: () => {} });
    expect(root.querySelector(".empty")).toBeNull();
  });

  it("defaults_scale_to_one_and_keeps_titles_as_text", () => {
    renderOverview(root, { monitorId: "M", tiles: [{ ...show.tiles[0], title: "<i>x</i>" }] }, { pick: () => {} });
    expect(tiles()[0].style.left).toBe("100px");
    expect(tiles()[0].querySelector(".title")!.innerHTML).toBe("&lt;i&gt;x&lt;/i&gt;");
  });
});
