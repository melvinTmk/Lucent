// Quick Look panel (Maj+Espace): preview of the selected result inside the search window.
// Pure DOM + one IPC call, so it is unit-tested with jsdom. Rust resolves the entry (the page
// never sends a path) and serves media through `qlook://` tokens that die on `quicklook_close`.
import { invoke } from "../shared/ipc";
import { t } from "../shared/i18n";
import type { QuickLookPreview, SearchEntry } from "../shared/types";

/** Result kinds that can be previewed. */
export function canPreview(entry: SearchEntry | undefined): entry is SearchEntry {
  return !!entry && (entry.kind === "file" || entry.kind === "project" || entry.kind === "app");
}

export function formatSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} o`;
  const units = ["Ko", "Mo", "Go", "To"];
  let v = bytes / 1024;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  return `${v < 10 ? v.toFixed(1).replace(".", ",") : Math.round(v)} ${units[i]}`;
}

export function formatDate(ms: number): string {
  return new Date(ms).toLocaleString("fr-FR", { dateStyle: "long", timeStyle: "short" });
}

/** Text as numbered monospace lines (no highlighting library). */
export function renderText(text: string): HTMLElement {
  const pre = document.createElement("div");
  pre.className = "ql-text";
  const lines = text.split("\n");
  const gutter = document.createElement("div");
  gutter.className = "ql-gutter";
  gutter.setAttribute("aria-hidden", "true");
  gutter.textContent = lines.map((_, i) => i + 1).join("\n");
  const code = document.createElement("pre");
  code.className = "ql-code";
  code.textContent = text;
  pre.append(gutter, code);
  return pre;
}

function card(entry: SearchEntry, p: QuickLookPreview): HTMLElement {
  const el = document.createElement("div");
  el.className = "ql-card";
  if (entry.icon) {
    const img = document.createElement("img");
    img.className = "ql-icon";
    img.src = entry.icon;
    img.alt = "";
    el.append(img);
  }
  const name = document.createElement("div");
  name.className = "ql-name";
  name.textContent = p.name ?? entry.title;
  el.append(name);
  const meta: string[] = [];
  if (p.size !== undefined) meta.push(`${t.search.quickLook.size} : ${formatSize(p.size)}`);
  if (p.modified !== undefined) meta.push(`${t.search.quickLook.modified} : ${formatDate(p.modified)}`);
  if (meta.length === 0 && entry.subtitle) meta.push(entry.subtitle);
  for (const m of meta) {
    const d = document.createElement("div");
    d.className = "ql-meta";
    d.textContent = m;
    el.append(d);
  }
  return el;
}

/** The preview body for `p` (`entry` supplies the icon and the fallback card text). */
export function renderPreview(entry: SearchEntry, p: QuickLookPreview): HTMLElement {
  if (p.kind === "text" && p.text !== undefined) return renderText(p.text);
  if (p.url && p.kind === "image") {
    const img = document.createElement("img");
    img.className = "ql-media";
    img.src = p.url;
    img.alt = entry.title;
    return img;
  }
  if (p.url && p.kind === "video") {
    const v = document.createElement("video");
    v.className = "ql-media";
    v.src = p.url;
    v.controls = true;
    v.autoplay = true;
    v.muted = true;
    return v;
  }
  if (p.url && p.kind === "pdf") {
    const f = document.createElement("iframe");
    f.className = "ql-media ql-pdf";
    f.src = p.url;
    f.title = entry.title;
    return f;
  }
  return card(entry, p);
}

export interface QuickLook {
  isOpen(): boolean;
  /** Opens the preview of `entry`, or closes it when already open. */
  toggle(entry: SearchEntry | undefined): Promise<void>;
  /** Shows the preview of `entry` while already open (selection moved). */
  refresh(entry: SearchEntry | undefined): Promise<void>;
  close(): void;
}

/** `host` is the preview container; `onChange` runs after it opens or closes. */
export function createQuickLook(host: HTMLElement, onChange: (open: boolean) => void): QuickLook {
  let open = false;
  let seq = 0;

  async function show(entry: SearchEntry): Promise<void> {
    const my = ++seq;
    let p: QuickLookPreview | null = null;
    try {
      p = await invoke("quicklook_open", { id: entry.id });
    } catch (e) {
      console.error("lucent: quicklook_open failed", e);
    }
    if (my !== seq) return; // closed or replaced meanwhile
    host.replaceChildren(renderPreview(entry, p ?? { kind: "none" }));
    if (!open) {
      open = true;
      host.hidden = false;
      onChange(true);
    }
  }

  function close(): void {
    seq++;
    if (!open) return;
    open = false;
    host.hidden = true;
    host.replaceChildren();
    void invoke("quicklook_close", {}).catch(() => {});
    onChange(false);
  }

  return {
    isOpen: () => open,
    async toggle(entry) {
      if (open) return close();
      if (canPreview(entry)) await show(entry);
    },
    async refresh(entry) {
      if (!open) return;
      if (canPreview(entry)) await show(entry);
      else close();
    },
    close,
  };
}
