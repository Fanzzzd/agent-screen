// Writes ?v=<hash of the file> after every /media/ URL and the og.png link card in every HTML file under public/.
// The Worker lets browsers keep versioned /media/ files for a year, so run this after any file in public/media/ changes.
// `node scripts/version-media.mjs --check` changes nothing and exits 1 if a version is stale.
import { createHash } from "node:crypto";
import { readdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const pub = new URL("../public/", import.meta.url);
const pubDir = fileURLToPath(pub);

function collectHtml(dir, out = []) {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const full = join(dir, entry.name);
    if (entry.isDirectory()) collectHtml(full, out);
    else if (entry.isFile() && entry.name.endsWith(".html")) out.push(full);
  }
  return out;
}

const pages = collectHtml(pubDir).sort();
const check = process.argv.includes("--check");
const stale = [];
let updated = 0;

for (const page of pages) {
  const rel = page.slice(pubDir.length);
  const before = readFileSync(page, "utf8");
  const after = before.replace(/(\/media\/[\w.-]+|\/og\.png)(\?[^"'\s>]*)?/g, (_, path) => {
    let bytes;
    try {
      bytes = readFileSync(new URL(path.slice(1), pub));
    } catch {
      console.error(`${rel} names ${path}, which is not in public/.`);
      process.exit(1);
    }
    return `${path}?v=${createHash("sha256").update(bytes).digest("hex").slice(0, 10)}`;
  });
  if (after !== before) {
    if (check) stale.push(rel);
    else {
      writeFileSync(page, after);
      updated += 1;
    }
  }
}

if (check) {
  if (stale.length) {
    console.error(`Stale /media/ or /og.png versions in: ${stale.join(", ")}. Run: node scripts/version-media.mjs`);
    process.exit(1);
  }
} else if (updated > 0) {
  console.log(`Updated /media/ and /og.png versions in ${updated} HTML file(s).`);
} else {
  console.log("The /media/ and /og.png versions in all HTML files are current.");
}
