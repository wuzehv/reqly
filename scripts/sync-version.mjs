import { readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import path from "node:path";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const version = JSON.parse(readFileSync(path.join(root, "package.json"), "utf8")).version;

if (!/^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$/.test(version)) {
  throw new Error(`Invalid semantic version: ${version}`);
}

function replaceVersion(file, pattern, label) {
  const content = readFileSync(file, "utf8");
  if (!pattern.test(content)) throw new Error(`Could not find ${label} version in ${file}`);
  writeFileSync(file, content.replace(pattern, `$1${version}$2`));
}

replaceVersion(
  path.join(root, "src-tauri", "Cargo.toml"),
  /(\[package\][\s\S]*?\nversion\s*=\s*")[^"]+("\s*\n)/,
  "Cargo package",
);
replaceVersion(
  path.join(root, "src-tauri", "Cargo.lock"),
  /(\[\[package\]\]\nname = "reqly"\nversion = ")[^"]+("\n)/,
  "Cargo lock package",
);

console.log(`Synchronized Reqly version ${version}`);
