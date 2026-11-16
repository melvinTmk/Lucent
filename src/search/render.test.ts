// @vitest-environment jsdom
import { beforeEach, describe, expect, it } from "vitest";
import type { EntryKind, SearchGroup, SearchHit } from "../shared/types";
import { GRID_COLS, highlight, layout, moveSelection, renderResults, splitGraphemes } from "./render";

function hit(id: string, kind: EntryKind, title: string, indices: number[] = [], subtitle = ""): SearchHit {
  return { entry: { id, kind, title, subtitle, icon: null, keywords: [] }, score: 1, indices };
}

function emoji(n: number): SearchHit[] {
  return Array.from({ length: n }, (_, i) => {
    const h = hit(`emoji:${i}`, "emoji", `😀 visage ${i}`);
    h.entry.keywords = ["😀"];
    return h;
  });
}

let root: HTMLElement;
beforeEach(() => {
  document.body.innerHTML = '<div id="results"></div>';
  root = document.getElementById("results")!;
});

const titles = () => [...root.querySelectorAll(".row .title")].map((e) => e.textContent);

describe("renderResults", () => {
  it("renders_groups_in_order_with_headers", () => {
    const groups: SearchGroup[] = [
      { key: "app", items: [hit("a1", "app", "Firefox"), hit("a2", "app", "Figma")] },
      { key: "project", items: [hit("p1", "project", "lucent", [], "C:\\dev")] },
      { key: "file", items: [hit("f1", "file", "notes.md")] },
    ];
    renderResults(root, "fi", layout(groups), 0);
    const sections = [...root.querySelectorAll<HTMLElement>(".group")];
    expect(sections.map((s) => s.dataset.key)).toEqual(["app", "project", "file"]);
    expect(sections.map((s) => s.querySelector(".header")!.textContent)).toEqual(["Applications", "Projets", "Fichiers"]);
    expect(titles()).toEqual(["Firefox", "Figma", "lucent", "notes.md"]);
    expect(root.querySelector(".row .sub")?.textContent).toBe("");
    expect(root.querySelectorAll(".row .sub")[2].textContent).toBe("C:\\dev");
    const selected = root.querySelectorAll('[aria-selected="true"]');
    expect(selected.length).toBe(1);
    expect(selected[0].querySelector(".title")!.textContent).toBe("Firefox");
  });

  it("bold_marks_match_indices", () => {
    const wrap = document.createElement("div");
    wrap.append(highlight("Firefox", [0, 1, 4]));
    expect(wrap.innerHTML).toBe("<b>Fi</b>re<b>f</b>ox");
    // Indices are grapheme positions: a decomposed é and a skin-toned emoji are one each.
    const g = document.createElement("div");
    g.append(highlight("e\u0301cole", [0]));
    expect(g.innerHTML).toBe("<b>e\u0301</b>cole");
    const e = document.createElement("div");
    e.append(highlight("👍🏽 ok", [2]));
    expect(e.innerHTML).toBe("👍🏽 <b>o</b>k");
    expect(splitGraphemes("👍🏽 ok")).toEqual(["👍🏽", " ", "o", "k"]);
    // Through the renderer, with HTML-looking titles kept as text.
    renderResults(root, "x", layout([{ key: "file", items: [hit("f", "file", "<i>x", [3])] }]), 0);
    expect(root.querySelector(".row .title")!.innerHTML).toBe("&lt;i&gt;<b>x</b>");
  });

  it("calculator_is_a_hero_row_at_the_top", () => {
    const groups: SearchGroup[] = [
      { key: "app", items: [hit("a1", "app", "Calculatrice")] },
      { key: "command", items: [hit("cmd:calc", "command", "= 42", [], "6*7 · Entrée : copier")] },
    ];
    const lay = layout(groups);
    expect(lay.hits.map((h) => h.entry.id)).toEqual(["cmd:calc", "a1"]);
    renderResults(root, "6*7", lay, 0);
    const hero = root.firstElementChild as HTMLElement;
    expect(hero.classList.contains("hero")).toBe(true);
    expect(hero.getAttribute("aria-selected")).toBe("true");
    expect(hero.querySelector(".sub")!.textContent).toBe("6*7 · Entrée : copier");
    expect(hero.textContent).toContain("42");
    // The command group, now empty, is gone.
    expect([...root.querySelectorAll<HTMLElement>(".group")].map((s) => s.dataset.key)).toEqual(["app"]);
  });

  it("rows_are_reused_when_ids_are_unchanged", () => {
    const g = (t: string, idx: number[]): SearchGroup[] => [{ key: "app", items: [hit("a1", "app", t, idx), hit("a2", "app", "Zed")] }];
    renderResults(root, "f", layout(g("Firefox", [0])), 0);
    const first = root.querySelector('[data-id="a1"]');
    renderResults(root, "fi", layout(g("Firefox", [0, 1])), 1);
    expect(root.querySelector('[data-id="a1"]')).toBe(first);
    expect(first!.querySelector(".title")!.innerHTML).toBe("<b>Fi</b>refox");
    expect(first!.getAttribute("aria-selected")).toBe("false");
    expect(root.querySelector('[data-id="a2"]')!.getAttribute("aria-selected")).toBe("true");
  });

  it("empty_query_without_results_is_blank_and_no_match_says_so", () => {
    renderResults(root, "", layout([]), 0);
    expect(root.textContent).toBe("");
    renderResults(root, "zzz", layout([]), 0);
    expect(root.querySelector(".empty")!.textContent).toBe("Aucun résultat");
    renderResults(root, "fi", layout([{ key: "app", items: [hit("a1", "app", "Firefox")] }]), 0);
    expect(root.querySelector(".empty")).toBeNull();
  });

  it("emoji_results_render_as_a_grid", () => {
    renderResults(root, ":vis", layout([{ key: "emoji", items: emoji(10) }]), 9);
    const cells = root.querySelectorAll(".grid .cell");
    expect(cells.length).toBe(10);
    expect(cells[0].textContent).toBe("😀");
    expect(cells[9].getAttribute("aria-selected")).toBe("true");
    expect(root.querySelector(".caption")!.textContent).toBe("visage 9");
  });
});

describe("moveSelection", () => {
  const list = layout([
    { key: "app", items: [hit("a1", "app", "A"), hit("a2", "app", "B")] },
    { key: "file", items: [hit("f1", "file", "C")] },
  ]);

  it("up_down_wrap_and_left_right_are_not_handled_in_lists", () => {
    expect(moveSelection(list, 0, "down")).toBe(1);
    expect(moveSelection(list, 2, "down")).toBe(0);
    expect(moveSelection(list, 0, "up")).toBe(2);
    expect(moveSelection(list, 0, "right")).toBeNull();
    expect(moveSelection(layout([]), 0, "down")).toBeNull();
  });

  it("grid_moves_in_two_dimensions_and_leaves_at_the_edges", () => {
    // command row (0), emoji grid of 20 (1..20), file row (21).
    const lay = layout([
      { key: "command", items: [hit("cmd:web", "command", "web"), hit("cmd:x", "command", "x")] },
      { key: "emoji", items: emoji(20) },
      { key: "file", items: [hit("f1", "file", "C")] },
    ]);
    expect(GRID_COLS).toBe(8);
    expect(lay.hits.length).toBe(23);
    const g0 = 2; // first emoji
    expect(moveSelection(lay, 1, "down")).toBe(g0);
    expect(moveSelection(lay, g0, "right")).toBe(g0 + 1);
    expect(moveSelection(lay, g0, "left")).toBe(g0);
    expect(moveSelection(lay, g0 + 3, "down")).toBe(g0 + 11);
    expect(moveSelection(lay, g0 + 11, "up")).toBe(g0 + 3);
    // Row 2 has only 4 cells: down from column 6 of row 1 lands on the last cell.
    expect(moveSelection(lay, g0 + 14, "down")).toBe(g0 + 19);
    // Last row: down leaves to the file row; up from the first row leaves to the command.
    expect(moveSelection(lay, g0 + 17, "down")).toBe(22);
    expect(moveSelection(lay, g0 + 2, "up")).toBe(1);
    // Up from the file row enters the grid on its last row.
    expect(moveSelection(lay, 22, "up")).toBe(g0 + 16);
  });
});

describe("valo tiles", () => {
  const valoHit = (id: string, valo: NonNullable<SearchHit["entry"]["valo"]>): SearchHit => {
    const h = hit(id, "valorant", `plain ${id}`, [], "sub");
    h.entry.valo = valo;
    return h;
  };
  const rank = (tierIcon: string | null) => valoHit("valo:rank", { type: "rank", tier: "Diamant 2", rr: 76, lastChange: 16, tierIcon });
  const match = (id: string, won: boolean | null, pics: boolean) =>
    valoHit(`valo:match:${id}`, {
      type: "match", won, score: [13, 10], map: "Bind", agent: "Fade", kda: [15, 16, 9], hsPct: pics ? 24 : null, acs: 231,
      competitive: true, mapImage: pics ? "http://valo.localhost/map-list-x.png" : null, agentIcon: pics ? "http://valo.localhost/agent-icon-x.png" : null,
    });
  const groups = (pics: boolean): SearchGroup[] => [
    { key: "valorant", items: [rank(pics ? "http://valo.localhost/tier-19.png" : null), match("a", true, pics), match("b", false, pics), match("c", null, pics)] },
  ];

  it("rank hero and match tiles with pictures", () => {
    renderResults(root, "valo", layout(groups(true)), 1);
    const hero = root.querySelector<HTMLElement>(".row.valo-rank")!;
    expect(hero.querySelector(".valo-tier-name")!.textContent).toBe("Diamant 2");
    expect(hero.querySelector<HTMLImageElement>(".valo-tier")!.getAttribute("src")).toBe("http://valo.localhost/tier-19.png");
    expect(hero.querySelector<HTMLElement>(".valo-bar-fill")!.style.width).toBe("76%");
    expect(hero.querySelector(".valo-delta-value")!.textContent).toBe("+16 RR");
    const tiles = [...root.querySelectorAll<HTMLElement>(".row.valo-tile")];
    expect(tiles.map((t) => t.querySelector(".valo-result")!.textContent)).toEqual(["VICTOIRE", "DÉFAITE", "ÉGALITÉ"]);
    expect(tiles.map((t) => ["win", "loss", "draw"].find((c) => t.classList.contains(c)))).toEqual(["win", "loss", "draw"]);
    expect(tiles[0].querySelector(".valo-score")!.textContent).toBe("13\u2009–\u200910");
    expect(tiles[0].querySelector(".valo-kda-value")!.textContent).toBe("15 / 16 / 9");
    expect(tiles[0].querySelector(".valo-hs .valo-kda-value")!.textContent).toBe("24 %");
    expect(tiles[0].querySelector<HTMLImageElement>(".valo-map")!.getAttribute("src")).toContain("map-list");
    expect(tiles[0].querySelector<HTMLImageElement>(".valo-agent-img")!.getAttribute("src")).toContain("agent-icon");
    // Selection model unchanged: index 1 = first tile.
    expect(tiles[0].getAttribute("aria-selected")).toBe("true");
    expect(tiles[0].dataset.index).toBe("1");
    const lay = layout(groups(true));
    expect(moveSelection(lay, 0, "down")).toBe(1);
    expect(moveSelection(lay, 3, "down")).toBe(0);
  });

  it("without pictures: glyph fallbacks, ACS instead of HS; pictures arriving reuse the rows", () => {
    renderResults(root, "valo", layout(groups(false)), 0);
    expect(root.querySelector(".valo-rank .valo-glyph")).not.toBeNull();
    expect(root.querySelector(".valo-tier")).toBeNull();
    const tile = root.querySelector<HTMLElement>(".row.valo-tile")!;
    expect(tile.querySelector(".valo-map")).toBeNull();
    expect(tile.querySelector(".valo-agent .valo-glyph")).not.toBeNull();
    expect(tile.querySelector(".valo-hs")!.textContent).toBe("ACS231");
    renderResults(root, "valo", layout(groups(true)), 0);
    expect(root.querySelector(".row.valo-tile")).toBe(tile);
    expect(tile.querySelector(".valo-map")).not.toBeNull();
    // Same data again: nothing rebuilt.
    const img = tile.querySelector(".valo-map");
    renderResults(root, "valo", layout(groups(true)), 2);
    expect(tile.querySelector(".valo-map")).toBe(img);
  });

  it("a plain row with the same id gets its standard layout back", () => {
    renderResults(root, "valo", layout(groups(true)), 0);
    const row = root.querySelector<HTMLElement>(".row.valo-rank")!;
    renderResults(root, "valo", layout([{ key: "valorant", items: [hit("valo:rank", "valorant", "Chargement…")] }]), 0);
    expect(row.classList.contains("valo-rank")).toBe(false);
    expect(row.querySelector(".title")!.textContent).toBe("Chargement…");
  });
});
