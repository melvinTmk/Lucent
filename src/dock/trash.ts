import { invoke } from "../shared/ipc";
import { t } from "../shared/i18n";
import type { TrashStatus } from "../shared/types";

const TRASH_EMPTY_SVG = `<svg viewBox="0 0 48 48" fill="none" xmlns="http://www.w3.org/2000/svg">
  <path d="M14 14V38C14 40.2091 15.7909 42 18 42H30C32.2091 42 34 40.2091 34 38V14" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"/>
  <path d="M10 14H38" stroke="currentColor" stroke-width="2.5" stroke-linecap="round"/>
  <path d="M19 14V10C19 8.89543 19.8954 8 21 8H27C28.1046 8 29 8.89543 29 10V14" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"/>
  <line x1="20" y1="20" x2="20" y2="34" stroke="currentColor" stroke-width="2" stroke-linecap="round" opacity="0.6"/>
  <line x1="24" y1="20" x2="24" y2="34" stroke="currentColor" stroke-width="2" stroke-linecap="round" opacity="0.6"/>
  <line x1="28" y1="20" x2="28" y2="34" stroke="currentColor" stroke-width="2" stroke-linecap="round" opacity="0.6"/>
</svg>`;

const TRASH_FULL_SVG = `<svg viewBox="0 0 48 48" fill="none" xmlns="http://www.w3.org/2000/svg">
  <path d="M14 14V38C14 40.2091 15.7909 42 18 42H30C32.2091 42 34 40.2091 34 38V14" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"/>
  <path d="M10 14H38" stroke="currentColor" stroke-width="2.5" stroke-linecap="round"/>
  <path d="M19 14V10C19 8.89543 19.8954 8 21 8H27C28.1046 8 29 8.89543 29 10V14" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"/>
  <!-- Crumpled paper texture -->
  <path d="M17 19L22 22L20 28L16 24Z" fill="currentColor" opacity="0.45"/>
  <path d="M26 18L31 22L29 27L24 23Z" fill="currentColor" opacity="0.55"/>
  <path d="M20 27L25 29L23 35L19 32Z" fill="currentColor" opacity="0.5"/>
  <path d="M27 26L32 29L30 35L25 33Z" fill="currentColor" opacity="0.4"/>
</svg>`;

let activeMenu: HTMLElement | null = null;

export function closeTrashMenu(): void {
  if (activeMenu) {
    const parent = activeMenu.parentElement;
    activeMenu.remove();
    activeMenu = null;
    parent?.dispatchEvent(new CustomEvent("trashlayout", { bubbles: true }));
  }
}

if (typeof document !== "undefined") {
  const onOutside = (e: Event) => {
    if (activeMenu && !activeMenu.contains(e.target as Node)) {
      closeTrashMenu();
    }
  };
  document.addEventListener("pointerdown", onOutside, true);
  document.addEventListener("click", onOutside, true);

  window.addEventListener("keydown", (e) => {
    if (e.key === "Escape" && activeMenu) {
      closeTrashMenu();
    }
  });
}

export function renderTrash(
  container: HTMLElement,
  status: TrashStatus | null,
  onEmpty?: () => void,
): HTMLButtonElement {
  let btn = container.querySelector<HTMLButtonElement>(".app.trash");
  if (!btn) {
    btn = document.createElement("button");
    btn.type = "button";
    btn.className = "app trash";
    btn.tabIndex = -1;
    container.appendChild(btn);
  }

  const isFull = (status?.count ?? 0) > 0;
  btn.classList.toggle("full", isFull);

  const label = isFull
    ? t.dock.trashFull.replace("%d", String(status!.count))
    : t.dock.trashEmpty;
  btn.setAttribute("aria-label", label);

  btn.innerHTML = isFull ? TRASH_FULL_SVG : TRASH_EMPTY_SVG;

  // Single click listener setup (idempotent)
  if (!btn.dataset.bound) {
    btn.dataset.bound = "true";

    btn.addEventListener("click", (e) => {
      // Left click opens the Recycle Bin folder in Windows Explorer
      if (e.button === 0) {
        closeTrashMenu();
        void invoke("open_external", { url: "shell:RecycleBinFolder" });
      }
    });

    btn.addEventListener("contextmenu", (e) => {
      e.preventDefault();
      e.stopPropagation();
      closeTrashMenu();

      const menu = document.createElement("div");
      menu.className = "island trash-menu";
      const emptyBtn = document.createElement("button");
      emptyBtn.type = "button";
      emptyBtn.className = "trash-menu-item";
      emptyBtn.textContent = t.dock.emptyTrash;
      emptyBtn.addEventListener("click", async () => {
        closeTrashMenu();
        await invoke("empty_trash");
        onEmpty?.();
      });

      menu.appendChild(emptyBtn);
      container.appendChild(menu);

      // Position menu relative to the trash button in container
      const pos = document.body.dataset.position ?? "bottom";
      menu.style.position = "absolute";
      if (pos === "top") {
        menu.style.top = "calc(100% + 10px)";
        menu.style.right = "0px";
      } else if (pos === "left") {
        menu.style.left = "calc(100% + 10px)";
        menu.style.bottom = "0px";
      } else if (pos === "right") {
        menu.style.right = "calc(100% + 10px)";
        menu.style.bottom = "0px";
      } else {
        menu.style.bottom = "calc(100% + 10px)";
        menu.style.right = "0px";
      }

      activeMenu = menu;
      container.dispatchEvent(new CustomEvent("trashlayout", { bubbles: true }));
    });
  }

  return btn;
}
