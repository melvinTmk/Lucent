import { invoke } from "../shared/ipc";
import { t } from "../shared/i18n";
import type { StackFile } from "../shared/types";

const STACK_ICON_SVG = `<svg viewBox="0 0 48 48" fill="none" xmlns="http://www.w3.org/2000/svg">
  <!-- macOS style folder silhouette -->
  <path d="M8 12C8 10.3431 9.34315 9 11 9H19L23 13H37C38.6569 13 40 14.3431 40 16V36C40 37.6569 38.6569 39 37 39H11C9.34315 39 8 37.6569 8 36V12Z" fill="currentColor" fill-opacity="0.16" stroke="currentColor" stroke-width="2.2" stroke-linejoin="round"/>
  <!-- Front flap / pocket -->
  <path d="M8 20C8 18.3431 9.34315 17 11 17H37C38.6569 17 40 18.3431 40 20V36C40 37.6569 38.6569 39 37 39H11C9.34315 39 8 37.6569 8 36V20Z" fill="currentColor" fill-opacity="0.28" stroke="currentColor" stroke-width="2.2" stroke-linejoin="round"/>
  <!-- Down arrow -->
  <circle cx="24" cy="28" r="7" fill="currentColor" fill-opacity="0.2"/>
  <path d="M21 27L24 30L27 27" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/>
  <path d="M24 23V30" stroke="currentColor" stroke-width="2" stroke-linecap="round"/>
</svg>`;

const FILE_ICON_SVG = `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
  <path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/>
  <polyline points="14 2 14 8 20 8"/>
</svg>`;

const FOLDER_ICON_SVG = `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
  <path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"/>
</svg>`;

let activePopover: HTMLElement | null = null;
let activeStackBtn: HTMLElement | null = null;

export function formatRelativeTime(modifiedMs: number): string {
  const diff = Math.max(0, Date.now() - modifiedMs);
  const sec = Math.floor(diff / 1000);
  const min = Math.floor(sec / 60);
  const hr = Math.floor(min / 60);
  const day = Math.floor(hr / 24);

  if (min < 1) return "À l'instant";
  if (min < 60) return `${min} min`;
  if (hr < 24) return `${hr} h`;
  if (day === 1) return "Hier";
  if (day < 7) return `${day} j`;
  const d = new Date(modifiedMs);
  return d.toLocaleDateString("fr-FR", { day: "numeric", month: "short" });
}

export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} o`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(0)} Ko`;
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} Mo`;
  return `${(bytes / (1024 * 1024 * 1024)).toFixed(1)} Go`;
}

export function closeDownloadsStack(): void {
  if (activePopover) {
    const parent = activePopover.parentElement;
    activePopover.remove();
    activePopover = null;
    activeStackBtn = null;
    parent?.dispatchEvent(new CustomEvent("stacklayout", { bubbles: true }));
  }
}

// Global dismiss listeners
if (typeof document !== "undefined") {
  const onOutside = (e: Event) => {
    if (activePopover) {
      const target = e.target as Node;
      if (!activePopover.contains(target) && (!activeStackBtn || !activeStackBtn.contains(target))) {
        closeDownloadsStack();
      }
    }
  };
  document.addEventListener("pointerdown", onOutside, true);
  document.addEventListener("click", onOutside, true);

  window.addEventListener("keydown", (e) => {
    if (e.key === "Escape" && activePopover) {
      closeDownloadsStack();
    }
  });
}

function positionPopover(popover: HTMLElement, anchor: HTMLElement, container: HTMLElement): void {
  const pos = document.body.dataset.position ?? "bottom";
  popover.style.position = "absolute";

  if (pos === "top") {
    popover.style.top = "calc(100% + 10px)";
    const appsRect = container.getBoundingClientRect();
    const btnRect = anchor.getBoundingClientRect();
    const rightOffset = Math.round(appsRect.right - btnRect.right + (btnRect.width / 2) - 130);
    popover.style.right = `${Math.max(0, rightOffset)}px`;
  } else if (pos === "left") {
    popover.style.left = "calc(100% + 10px)";
    const appsRect = container.getBoundingClientRect();
    const btnRect = anchor.getBoundingClientRect();
    const topOffset = Math.round(btnRect.top + btnRect.height / 2 - appsRect.top - 150);
    popover.style.top = `${Math.max(0, topOffset)}px`;
  } else if (pos === "right") {
    popover.style.right = "calc(100% + 10px)";
    const appsRect = container.getBoundingClientRect();
    const btnRect = anchor.getBoundingClientRect();
    const topOffset = Math.round(btnRect.top + btnRect.height / 2 - appsRect.top - 150);
    popover.style.top = `${Math.max(0, topOffset)}px`;
  } else {
    // Bottom dock default
    popover.style.bottom = "calc(100% + 10px)";
    const appsRect = container.getBoundingClientRect();
    const btnRect = anchor.getBoundingClientRect();
    const rightOffset = Math.round(appsRect.right - btnRect.right + (btnRect.width / 2) - 130);
    popover.style.right = `${Math.max(0, rightOffset)}px`;
  }
}

export function renderDownloadsStack(container: HTMLElement): HTMLButtonElement {
  let btn = container.querySelector<HTMLButtonElement>(".app.stack");
  if (!btn) {
    btn = document.createElement("button");
    btn.type = "button";
    btn.className = "app stack";
    btn.tabIndex = -1;
    container.appendChild(btn);
  }

  btn.setAttribute("aria-label", t.dock.downloads);
  btn.innerHTML = STACK_ICON_SVG;

  if (!btn.dataset.bound) {
    btn.dataset.bound = "true";

    btn.addEventListener("dblclick", (e) => {
      e.stopPropagation();
      closeDownloadsStack();
      void invoke("open_external", { url: "shell:Downloads" });
    });

    btn.addEventListener("click", async (e) => {
      e.stopPropagation();

      if (activePopover) {
        closeDownloadsStack();
        return;
      }

      const popover = document.createElement("div");
      popover.className = "island stack-popover";
      container.appendChild(popover);
      activePopover = popover;
      activeStackBtn = btn;
      positionPopover(popover, btn!, container);
      container.dispatchEvent(new CustomEvent("stacklayout", { bubbles: true }));

      try {
        const files: StackFile[] = await invoke("list_stack_files", { folder: "downloads", limit: 8 });

        // Check if still open
        if (activePopover !== popover) return;

        popover.innerHTML = "";

        const header = document.createElement("div");
        header.className = "stack-header";
        header.textContent = t.dock.downloads;
        popover.appendChild(header);

        const list = document.createElement("div");
        list.className = "stack-list";

        if (files.length === 0) {
          const empty = document.createElement("div");
          empty.className = "stack-empty";
          empty.textContent = t.dock.downloadsEmpty;
          list.appendChild(empty);
        } else {
          for (const file of files) {
            const item = document.createElement("button");
            item.type = "button";
            item.className = "stack-item";

            const iconSpan = document.createElement("span");
            iconSpan.className = "stack-item-icon";
            iconSpan.innerHTML = file.isDir ? FOLDER_ICON_SVG : FILE_ICON_SVG;

            const metaSpan = document.createElement("span");
            metaSpan.className = "stack-item-meta";

            const nameSpan = document.createElement("span");
            nameSpan.className = "stack-item-name";
            nameSpan.textContent = file.name;

            const detailSpan = document.createElement("span");
            detailSpan.className = "stack-item-detail";
            detailSpan.textContent = file.isDir
              ? formatRelativeTime(file.modified)
              : `${formatRelativeTime(file.modified)} · ${formatBytes(file.size)}`;

            metaSpan.appendChild(nameSpan);
            metaSpan.appendChild(detailSpan);

            item.appendChild(iconSpan);
            item.appendChild(metaSpan);

            item.addEventListener("click", (evt) => {
              evt.stopPropagation();
              closeDownloadsStack();
              void invoke("open_external", { url: file.path });
            });

            list.appendChild(item);
          }
        }
        popover.appendChild(list);

        const footer = document.createElement("div");
        footer.className = "stack-footer";
        const footerBtn = document.createElement("button");
        footerBtn.type = "button";
        footerBtn.className = "stack-footer-btn";
        footerBtn.textContent = t.dock.openInExplorer;
        footerBtn.addEventListener("click", (evt) => {
          evt.stopPropagation();
          closeDownloadsStack();
          void invoke("open_external", { url: "shell:Downloads" });
        });
        footer.appendChild(footerBtn);
        popover.appendChild(footer);

        positionPopover(popover, btn!, container);
        container.dispatchEvent(new CustomEvent("stacklayout", { bubbles: true }));
      } catch (err) {
        if (activePopover === popover) {
          closeDownloadsStack();
        }
      }
    });
  }

  return btn;
}
