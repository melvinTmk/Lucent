// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const { invoke } = vi.hoisted(() => ({
  invoke: vi.fn(async (_cmd: string, ..._rest: unknown[]) => {}),
}));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));

import { disposeCard, renderCard, type CardViewState } from "./card";

describe("card component", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    invoke.mockClear();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("renders a victory match card with win class and dismisses after duration", () => {
    const el = document.createElement("div");
    const onDismiss = vi.fn();
    const state: CardViewState = {
      kind: "match",
      line: "VICTOIRE 13-10 · Ascent · 20/10/5 · +18 RR → Platine 2",
      won: true,
    };

    renderCard(el, state, { durationMs: 4000, onDismiss });

    expect(el.classList.contains("island")).toBe(true);
    expect(el.classList.contains("card")).toBe(true);
    expect(el.classList.contains("win")).toBe(true);
    expect(el.textContent).toContain("VICTOIRE 13-10 · Ascent");

    vi.advanceTimersByTime(3999);
    expect(onDismiss).not.toHaveBeenCalled();

    vi.advanceTimersByTime(1);
    expect(onDismiss).toHaveBeenCalledTimes(1);
  });

  it("renders a loss match card with loss class", () => {
    const el = document.createElement("div");
    const state: CardViewState = {
      kind: "match",
      line: "DÉFAITE 8-13 · Bind · 12/15/4",
      won: false,
    };

    renderCard(el, state, { durationMs: 3000 });

    expect(el.classList.contains("card")).toBe(true);
    expect(el.classList.contains("loss")).toBe(true);
    expect(el.textContent).toContain("DÉFAITE 8-13 · Bind");
  });

  it("renders a draw match card with draw class", () => {
    const el = document.createElement("div");
    const state: CardViewState = {
      kind: "match",
      line: "ÉGALITÉ 12-12 · Haven · 15/15/8",
      won: null,
    };

    renderCard(el, state);

    expect(el.classList.contains("card")).toBe(true);
    expect(el.classList.contains("draw")).toBe(true);
    expect(el.textContent).toContain("ÉGALITÉ 12-12 · Haven");
  });

  it("renders ask prompt card and clicking buttons calls game_answer and dismisses", () => {
    const el = document.createElement("div");
    const onDismiss = vi.fn();
    const state: CardViewState = {
      kind: "ask",
      exe: "c:\\games\\mygame.exe",
      name: "My Game",
    };

    renderCard(el, state, { onDismiss });

    expect(el.classList.contains("ask")).toBe(true);
    expect(el.textContent).toContain("My Game");

    const buttons = el.querySelectorAll("button");
    expect(buttons.length).toBe(2);

    const yesBtn = Array.from(buttons).find((b) => b.textContent?.includes("Oui"));
    const noBtn = Array.from(buttons).find((b) => b.textContent?.includes("Non"));
    expect(yesBtn).toBeDefined();
    expect(noBtn).toBeDefined();

    // Click Yes
    yesBtn?.click();
    expect(invoke).toHaveBeenCalledWith("game_answer", { exe: "c:\\games\\mygame.exe", isGame: true });
    expect(onDismiss).toHaveBeenCalledTimes(1);

    // Render again and click No
    invoke.mockClear();
    onDismiss.mockClear();
    renderCard(el, state, { onDismiss });
    const buttons2 = el.querySelectorAll("button");
    const noBtn2 = Array.from(buttons2).find((b) => b.textContent?.includes("Non"));
    noBtn2?.click();
    expect(invoke).toHaveBeenCalledWith("game_answer", { exe: "c:\\games\\mygame.exe", isGame: false });
    expect(onDismiss).toHaveBeenCalledTimes(1);
  });

  it("mic card says « Micro coupé » / « Micro activé » and leaves after 1.5 s", () => {
    const el = document.createElement("div");
    const onDismiss = vi.fn();
    renderCard(el, { kind: "mic", muted: true }, { durationMs: 4000, onDismiss });
    expect(el.classList.contains("card")).toBe(true);
    expect(el.classList.contains("mic")).toBe(true);
    expect(el.classList.contains("muted")).toBe(true);
    expect(el.textContent).toBe("Micro coupé");
    vi.advanceTimersByTime(1000);
    // A re-render of the same state keeps the running timer.
    renderCard(el, { kind: "mic", muted: true }, { durationMs: 4000, onDismiss });
    vi.advanceTimersByTime(499);
    expect(onDismiss).not.toHaveBeenCalled();
    vi.advanceTimersByTime(1);
    expect(onDismiss).toHaveBeenCalledTimes(1);

    renderCard(el, { kind: "mic", muted: false }, { onDismiss });
    expect(el.classList.contains("muted")).toBe(false);
    expect(el.textContent).toBe("Micro activé");
    vi.advanceTimersByTime(1500);
    expect(onDismiss).toHaveBeenCalledTimes(2);
  });

  const rich = {
    kind: "match" as const,
    line: "VICTOIRE 13-10 · Bind · 15/16/9 · +16 RR → Diamant 2",
    won: true,
    map: "Bind",
    agent: "Fade",
    competitive: true,
    score: [13, 10] as [number, number],
    kda: [15, 16, 9] as [number, number, number],
    hsPct: 24,
    acs: 231,
    rrChange: 16,
    tier: "Diamant 2",
    tierId: 19,
    mapSplash: "http://valo.localhost/map-splash-x.png",
    agentPortrait: "http://valo.localhost/agent-portrait-x.png",
    tierIcon: "http://valo.localhost/tier-19.png",
  };

  it("pinned Valorant card: no dismissal, pictures kept across re-renders", () => {
    const el = document.createElement("div");
    const onDismiss = vi.fn();
    renderCard(el, rich, { onDismiss });
    const img = el.querySelector(".vc-splash");
    vi.advanceTimersByTime(60_000);
    renderCard(el, { ...rich }, { onDismiss });
    expect(el.querySelector(".vc-splash")).toBe(img);
    expect(onDismiss).not.toHaveBeenCalled();
    disposeCard();
  });

  it("Valorant card: progress to the next rank only when known", () => {
    const el = document.createElement("div");
    renderCard(el, { ...rich, rr: 76, nextTier: "Diamant 3", rrToNext: 24 });
    expect(el.querySelector(".vc-next-text")!.textContent).toBe("24 RR avant Diamant 3");
    expect(el.querySelector<HTMLElement>(".vc-next-fill")!.style.width).toBe("76%");
    disposeCard();
    const top = document.createElement("div");
    renderCard(top, { ...rich, rr: 150, nextTier: null, rrToNext: null });
    expect(top.querySelector(".vc-next")).toBeNull();
    disposeCard();
  });

  it("Valorant card with pictures: splash, portrait, result, score, stats, RR", () => {
    const el = document.createElement("div");
    const onDismiss = vi.fn();
    renderCard(el, rich, { durationMs: 4000, onDismiss });
    expect(el.classList.contains("valo")).toBe(true);
    expect(el.classList.contains("win")).toBe(true);
    expect(el.querySelector<HTMLImageElement>(".vc-splash")!.getAttribute("src")).toBe(rich.mapSplash);
    expect(el.querySelector<HTMLImageElement>(".vc-agent-img")!.getAttribute("src")).toBe(rich.agentPortrait);
    expect(el.querySelector(".vc-kicker")!.textContent).toBe("Fade · Bind");
    expect(el.querySelector(".vc-result")!.textContent).toBe("VICTOIRE");
    expect(el.querySelector(".vc-score")!.textContent).toBe("13\u2009–\u200910");
    expect([...el.querySelectorAll(".vc-stat-value")].map((n) => n.textContent)).toEqual(["15/16/9", "24%", "231"]);
    expect(el.querySelector(".vc-rr")!.textContent).toBe("+16 RR");
    expect(el.querySelector(".vc-rr")!.classList.contains("up")).toBe(true);
    expect(el.querySelector(".vc-tier-name")!.textContent).toBe("Diamant 2");
    expect(el.querySelector<HTMLImageElement>(".vc-tier")!.getAttribute("src")).toBe(rich.tierIcon);
    // A re-render with the same card keeps its pictures and its dismissal time.
    const img = el.querySelector(".vc-splash");
    vi.advanceTimersByTime(3000);
    renderCard(el, { ...rich }, { durationMs: 4000, onDismiss });
    expect(el.querySelector(".vc-splash")).toBe(img);
    vi.advanceTimersByTime(1000);
    expect(onDismiss).toHaveBeenCalledTimes(1);
  });

  it("Valorant card: loss without RR, missing portrait, failed picture removed", () => {
    const el = document.createElement("div");
    renderCard(el, { ...rich, won: false, competitive: false, rrChange: null, tier: null, agentPortrait: null, hsPct: null });
    expect(el.classList.contains("loss")).toBe(true);
    expect(el.classList.contains("has-agent")).toBe(false);
    expect(el.querySelector(".vc-agent")).toBeNull();
    expect(el.querySelector(".vc-result")!.textContent).toBe("DÉFAITE");
    expect(el.querySelector(".vc-kicker")!.textContent).toBe("Fade · Bind · Partie");
    expect(el.querySelector(".vc-rank")).toBeNull();
    expect([...el.querySelectorAll(".vc-stat-label")].map((n) => n.textContent)).toEqual(["K/D/A", "ACS"]);
    el.querySelector(".vc-splash")!.dispatchEvent(new Event("error"));
    expect(el.querySelector(".vc-splash")).toBeNull();
  });

  it("Valorant card without any picture keeps the text line", () => {
    const el = document.createElement("div");
    renderCard(el, { ...rich, mapSplash: null, agentPortrait: null, tierIcon: null });
    expect(el.classList.contains("valo")).toBe(false);
    expect(el.textContent).toBe(rich.line);
    // Then a rich card on the same element, then the text one again: no leftovers.
    renderCard(el, rich);
    expect(el.classList.contains("valo")).toBe(true);
    renderCard(el, { kind: "match", line: "DÉFAITE 8-13", won: false });
    expect(el.classList.contains("valo")).toBe(false);
    expect(el.querySelector(".vc-bg")).toBeNull();
    expect(el.textContent).toBe("DÉFAITE 8-13");
  });
});
