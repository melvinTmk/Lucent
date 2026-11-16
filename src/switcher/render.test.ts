// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { SwitcherApp, SwitcherShow, SwitcherWindows } from "../shared/types";
import { CELL, PAD, renderSwitcher, renderWindows, selectApp, selectWindow } from "./render";

function app(id: string, name: string, hwnds: number[] = [1]): SwitcherApp {
  return { id, name, iconUrl: `icon://${id}`, windows: hwnds.map((h) => ({ hwnd: h, title: `w${h}` })) };
}

const show: SwitcherShow = { apps: [app("code", "Code"), app("ff", "Firefox"), app("term", "Terminal")], index: 1 };

let root: HTMLElement;
beforeEach(() => {
  document.body.innerHTML = '<div id="island"></div>';
  root = document.getElementById("island")!;
});

const slots = () => [...root.querySelectorAll<HTMLElement>(".slot")];

describe("renderSwitcher", () => {
  it("renders_one_slot_per_app_in_mru_order", () => {
    renderSwitcher(root, show, () => {});
    expect(slots().map((s) => s.dataset.id)).toEqual(["code", "ff", "term"]);
    expect(slots().map((s) => s.querySelector("img")!.getAttribute("src"))).toEqual([
      "icon://code",
      "icon://ff",
      "icon://term",
    ]);
    expect(slots().map((s) => s.getAttribute("aria-label"))).toEqual(["Code", "Firefox", "Terminal"]);
    expect(root.getAttribute("role")).toBe("listbox");
  });

  it("shows_the_selected_app_name_only", () => {
    renderSwitcher(root, show, () => {});
    expect(root.querySelector(".name")!.textContent).toBe("Firefox");
    expect(slots().map((s) => s.getAttribute("aria-selected"))).toEqual(["false", "true", "false"]);
    const sel = root.querySelector<HTMLElement>(".sel")!;
    expect(sel.style.transform).toBe(`translateX(${PAD + CELL}px)`);
  });

  it("select_moves_plate_and_name", () => {
    renderSwitcher(root, show, () => {});
    selectApp(root, 2);
    expect(root.querySelector(".name")!.textContent).toBe("Terminal");
    expect(slots()[2].getAttribute("aria-selected")).toBe("true");
    expect(root.querySelector<HTMLElement>(".sel")!.style.transform).toBe(`translateX(${PAD + 2 * CELL}px)`);
    selectApp(root, 9);
    // Out of range: unchanged.
    expect(root.querySelector(".name")!.textContent).toBe("Terminal");
  });

  it("uses_the_cell_size_from_rust", () => {
    renderSwitcher(root, { ...show, cell: 44 }, () => {});
    expect(slots()[1].style.left).toBe(`${PAD + 44}px`);
    expect(slots()[1].style.width).toBe("44px");
    expect(root.querySelector<HTMLElement>(".slot img")!.style.width).toBe("32px");
  });

  it("click_picks_the_app", () => {
    const pick = vi.fn();
    renderSwitcher(root, show, pick);
    slots()[2].click();
    expect(pick).toHaveBeenCalledWith("term", undefined);
  });

  it("names_are_text_not_html", () => {
    renderSwitcher(root, { apps: [app("x", "<b>x</b>")], index: 0 }, () => {});
    expect(root.querySelector(".name")!.innerHTML).toBe("&lt;b&gt;x&lt;/b&gt;");
  });
});

describe("renderWindows", () => {
  const w: SwitcherWindows = {
    appIndex: 1,
    hwnds: [21, 22],
    titles: ["Inbox", "  "],
    rects: [
      { x: 30, y: 200, w: 300, h: 150 },
      { x: 360, y: 200, w: 200, h: 150 },
    ],
    index: 1,
    scale: 1.5,
    titleH: 36,
  };

  it("renders_cards_with_titles_and_selection", () => {
    const pick = vi.fn();
    renderSwitcher(root, { apps: [app("code", "Code"), app("ff", "Firefox", [21, 22])], index: 1 }, pick);
    renderWindows(root, w, pick);
    const cards = [...root.querySelectorAll<HTMLElement>(".win")];
    expect(cards.map((c) => c.querySelector(".title")!.textContent)).toEqual(["Inbox", "Sans titre"]);
    expect(cards.map((c) => c.getAttribute("aria-selected"))).toEqual(["false", "true"]);
    expect(cards[0].style.left).toBe(`${30 / 1.5 - 8}px`);
    expect(cards[0].style.width).toBe(`${300 / 1.5 + 16}px`);
    selectWindow(root, 0);
    expect(cards.map((c) => c.getAttribute("aria-selected"))).toEqual(["true", "false"]);
    cards[1].click();
    expect(pick).toHaveBeenCalledWith("ff", 22);
  });

  it("null_clears_the_row", () => {
    renderSwitcher(root, show, () => {});
    renderWindows(root, { ...w, appIndex: 1 }, () => {});
    renderWindows(root, null, () => {});
    expect(root.querySelectorAll(".win").length).toBe(0);
    expect(root.classList.contains("expanded")).toBe(false);
  });
});
