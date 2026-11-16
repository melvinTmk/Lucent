import { invoke } from "../shared/ipc";
import { t } from "../shared/i18n";
import type { ValoMatchCard } from "../shared/types";

/** The mic card (after a mic mute toggle) stays this long, whatever the card setting. */
export const MIC_CARD_MS = 1500;
/** The shared-monitor card (after an input switch). */
export const DISPLAY_CARD_MS = 2200;

export type CardViewState =
  | ({ kind: "match" } & ValoMatchCard)
  | { kind: "ask"; exe: string; name: string }
  | { kind: "mic"; muted: boolean }
  | { kind: "display"; text: string; ok: boolean };

export interface CardOptions {
  durationMs?: number;
  onDismiss?: () => void;
}

let dismissTimer: ReturnType<typeof setTimeout> | null = null;

export function disposeCard(): void {
  if (dismissTimer !== null) {
    clearTimeout(dismissTimer);
    dismissTimer = null;
  }
}

export function renderCard(
  el: HTMLElement,
  state: CardViewState,
  options?: CardOptions,
): void {
  // Re-renders (clock, events) must not push the mic card's dismissal back.
  if (state.kind === "mic" && dismissTimer !== null && el.dataset.mic === String(state.muted)) return;
  // Same for the match card (and its pictures are not rebuilt).
  const sig = state.kind === "match" ? JSON.stringify(state) : undefined;
  if (sig !== undefined && el.dataset.match === sig && (dismissTimer !== null || !options?.durationMs)) return;
  disposeCard();
  delete el.dataset.mic;
  delete el.dataset.match;

  el.className = "island card";

  if (state.kind === "match") {
    const statusClass = state.won === true ? "win" : state.won === false ? "loss" : "draw";
    el.classList.add(statusClass);
    if (isRich(state)) renderValoCard(el, state);
    else el.textContent = state.line;

    el.dataset.match = sig;
    if (options?.durationMs && options.onDismiss) {
      dismissTimer = setTimeout(() => {
        dismissTimer = null;
        options.onDismiss?.();
      }, options.durationMs);
    }
  } else if (state.kind === "display") {
    el.classList.add("display-switch");
    el.classList.toggle("failed", !state.ok);
    el.textContent = state.text;
    dismissTimer = setTimeout(() => {
      dismissTimer = null;
      options?.onDismiss?.();
    }, DISPLAY_CARD_MS);
  } else if (state.kind === "mic") {
    el.classList.add("mic");
    el.classList.toggle("muted", state.muted);
    el.dataset.mic = String(state.muted);
    el.textContent = state.muted ? t.mic.muted : t.mic.unmuted;
    dismissTimer = setTimeout(() => {
      dismissTimer = null;
      options?.onDismiss?.();
    }, MIC_CARD_MS);
  } else if (state.kind === "ask") {
    el.classList.add("ask");
    el.innerHTML = "";

    const text = document.createElement("span");
    text.className = "card-prompt";
    text.textContent = `Considérer ${state.name} comme un jeu ?`;
    el.appendChild(text);

    const yesBtn = document.createElement("button");
    yesBtn.type = "button";
    yesBtn.className = "card-btn primary";
    yesBtn.textContent = "Oui, c'est un jeu";
    yesBtn.onclick = () => {
      void invoke("game_answer", { exe: state.exe, isGame: true });
      options?.onDismiss?.();
    };
    el.appendChild(yesBtn);

    const noBtn = document.createElement("button");
    noBtn.type = "button";
    noBtn.className = "card-btn";
    noBtn.textContent = "Non";
    noBtn.onclick = () => {
      void invoke("game_answer", { exe: state.exe, isGame: false });
      options?.onDismiss?.();
    };
    el.appendChild(noBtn);
  }
}

// ---------- Valorant post-match card ----------

/** CSS px the agent portrait rises above the card. The compact card keeps it inside. */
export const VALO_PORTRAIT_OVERFLOW = 0;

/** The Valorant layout needs at least one picture; offline, the text line stays. */
function isRich(c: ValoMatchCard): boolean {
  return !!(c.mapSplash || c.agentPortrait);
}

function node<K extends keyof HTMLElementTagNameMap>(tag: K, className: string, text?: string): HTMLElementTagNameMap[K] {
  const n = document.createElement(tag);
  n.className = className;
  if (text !== undefined) n.textContent = text;
  return n;
}

/** `<img>` that removes itself when the picture fails (the dark panel stays). */
function picture(className: string, src: string): HTMLImageElement {
  const img = node("img", className);
  img.alt = "";
  img.draggable = false;
  img.decoding = "async";
  img.addEventListener("error", () => img.remove());
  img.src = src;
  return img;
}

export function resultLabel(won: boolean | null): string {
  return won === true ? "VICTOIRE" : won === false ? "DÉFAITE" : "ÉGALITÉ";
}

function stat(label: string, value: string): HTMLElement {
  const s = node("div", "vc-stat");
  s.append(node("span", "vc-stat-label", label), node("span", "vc-stat-value", value));
  return s;
}

function renderValoCard(el: HTMLElement, c: ValoMatchCard): void {
  el.classList.add("valo");
  el.classList.toggle("has-agent", !!c.agentPortrait);
  el.replaceChildren();

  const bg = node("div", "vc-bg");
  if (c.mapSplash) bg.append(picture("vc-splash", c.mapSplash));
  bg.append(node("div", "vc-shade"), node("div", "vc-stripe"));
  el.append(bg);

  if (c.agentPortrait) {
    const wrap = node("div", "vc-agent");
    wrap.append(picture("vc-agent-img", c.agentPortrait));
    el.append(wrap);
  }

  // Result: "JETT · ASCENT" over "VICTOIRE 13–9".
  const main = node("div", "vc-col vc-main");
  const kicker = [c.agent, c.map, c.competitive ? null : "Partie"].filter(Boolean).join(" · ");
  main.append(node("div", "vc-kicker", kicker));
  const head = node("div", "vc-head");
  head.append(node("span", "vc-result", resultLabel(c.won)));
  if (c.score) head.append(node("span", "vc-score", `${c.score[0]}\u2009–\u2009${c.score[1]}`));
  main.append(head);
  el.append(main);

  const stats = node("div", "vc-stats");
  if (c.kda) stats.append(stat("K/D/A", `${c.kda[0]}/${c.kda[1]}/${c.kda[2]}`));
  if (c.hsPct != null) stats.append(stat("HS", `${c.hsPct}%`));
  if (c.acs != null) stats.append(stat("ACS", String(c.acs)));
  if (stats.childElementCount > 0) el.append(node("div", "vc-sep"), stats);

  // Rank: emblem, "+21 RR ASCENDANT 2" over the bar and "20 RR avant Ascendant 3".
  if (c.rrChange != null && c.tier) {
    const rank = node("div", "vc-rank");
    if (c.tierIcon) rank.append(picture("vc-tier", c.tierIcon));
    const col = node("div", "vc-col");
    const line = node("div", "vc-rank-line");
    const delta = node("span", "vc-rr", `${c.rrChange > 0 ? "+" : c.rrChange < 0 ? "−" : "±"}${Math.abs(c.rrChange)} RR`);
    delta.classList.add(c.rrChange > 0 ? "up" : c.rrChange < 0 ? "down" : "flat");
    line.append(delta, node("span", "vc-tier-name", c.tier));
    col.append(line);
    if (c.nextTier && c.rrToNext != null && c.rr != null) {
      const next = node("div", "vc-next");
      const bar = node("div", "vc-next-bar");
      const fill = node("div", "vc-next-fill");
      fill.style.width = `${Math.max(0, Math.min(100, c.rr))}%`;
      bar.append(fill);
      next.append(bar, node("span", "vc-next-text", `${c.rrToNext} RR avant ${c.nextTier}`));
      col.append(next);
    }
    rank.append(col);
    el.append(node("div", "vc-sep"), rank);
  }
}
