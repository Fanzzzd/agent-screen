// Writes ?v=<hash of the file> after every /media/ URL in public/index.html.
// The Worker lets browsers keep versioned /media/ files for a year, so run this after any file in public/media/ changes.
// `node scripts/version-media.mjs --check` changes nothing and exits 1 if a version is stale.
import { createHash } from "node:crypto";
import { readFileSync, writeFileSync } from "node:fs";

const pub = new URL("../public/", import.meta.url);
const page = new URL("index.html", pub);
const before = readFileSync(page, "utf8");
const after = before.replace(/(\/media\/[\w.-]+)(\?v=[0-9a-f]*)?/g, (_, path) => {
  const hash = createHash("sha256").update(readFileSync(new URL(path.slice(1), pub))).digest("hex");
  return `${path}?v=${hash.slice(0, 10)}`;
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
