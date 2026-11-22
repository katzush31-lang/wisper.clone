// Sets the app version in tauri.conf.json. Usage: node scripts/set-version.mjs 0.1.42
import { readFileSync, writeFileSync } from "node:fs";

const version = process.argv[2];
if (!/^\d+\.\d+\.\d+$/.test(version ?? "")) {
  console.error("usage: set-version.mjs <major.minor.patch>");
  process.exit(1);
}
const path = "src-tauri/tauri.conf.json";
const conf = JSON.parse(readFileSync(path, "utf8"));
conf.version = version;
writeFileSync(path, JSON.stringify(conf, null, 2) + "\n");
console.log(`version set to ${version}`);
