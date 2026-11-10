// Writes ?v=<hash of the file> after every /media/ URL in public/index.html.
// The Worker lets browsers keep versioned /media/ files for a year, so run this after any file in public/media/ changes.
// `node scripts/version-media.mjs --check` changes nothing and exits 1 if a version is stale.
import { createHash } from "node:crypto";
import { readFileSync, writeFileSync } from "node:fs";

const pub = new URL("../public/", import.meta.url);
const page = new URL("index.html", pub);
const before = readFileSync(page, "utf8");
const after = before.replace(/(\/media\/[\w.-]+)(\?[^"'\s>]*)?/g, (_, path) => {
  let bytes;
  try {
    bytes = readFileSync(new URL(path.slice(1), pub));
  } catch {
    console.error(`index.html names ${path}, which is not in public/.`);
    process.exit(1);
  }
  return `${path}?v=${createHash("sha256").update(bytes).digest("hex").slice(0, 10)}`;
});
if (process.argv.includes("--check")) {
  if (after !== before) {
    console.error("index.html has stale /media/ versions. Run: node scripts/version-media.mjs");
    process.exit(1);
  }
} else if (after !== before) {
  writeFileSync(page, after);
  console.log("Updated the /media/ versions in index.html.");
} else {
  console.log("The /media/ versions in index.html are current.");
}
