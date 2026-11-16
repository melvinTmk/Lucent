// Generates src-tauri/src/search/emoji.json from emojibase-data (fr + en, compact format).
// Output: [{ "e": "😀", "k": ["visage souriant", "grinning", ...] }]. Skin-tone variants
// (nested under `skins` in emojibase) are dropped; so are group-less entries (regional
// indicator letters) and the "component" group (skin swatches, hair styles).
import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const load = (lang) =>
  JSON.parse(readFileSync(join(root, "node_modules", "emojibase-data", lang, "compact.json"), "utf8"));

const COMPONENT_GROUP = 2;
const en = new Map(load("en").map((x) => [x.hexcode, x]));
const out = [];
for (const fr of load("fr")) {
  if (fr.group === undefined || fr.group === COMPONENT_GROUP) continue;
  const words = new Set();
  for (const src of [fr, en.get(fr.hexcode)]) {
    if (!src) continue;
    for (const w of [src.label, ...(src.tags ?? [])]) {
      const t = w?.trim().toLowerCase();
      if (t) words.add(t);
    }
  }
  out.push({ e: fr.unicode, k: [...words] });
}
const target = join(root, "src-tauri", "src", "search", "emoji.json");
writeFileSync(target, JSON.stringify(out));
console.log(`${out.length} emojis -> ${target}`);
