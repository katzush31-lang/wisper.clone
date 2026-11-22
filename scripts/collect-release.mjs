// Gathers the built installer into ./release with a stable name (Wisper-Setup.exe) and,
// when the build was signed, writes latest.json for the in-app updater.
// Usage: node scripts/collect-release.mjs <version> <owner/repo>
import { copyFileSync, existsSync, mkdirSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const [version, repo] = process.argv.slice(2);
if (!version || !repo) {
  console.error("usage: collect-release.mjs <version> <owner/repo>");
  process.exit(1);
}
const nsisDir = "target/release/bundle/nsis";
const exe = readdirSync(nsisDir).find((f) => f.endsWith("-setup.exe") && f.includes(version));
if (!exe) {
  console.error(`no installer for ${version} in ${nsisDir}: ${readdirSync(nsisDir).join(", ")}`);
  process.exit(1);
}
mkdirSync("release", { recursive: true });
copyFileSync(join(nsisDir, exe), join("release", exe));
copyFileSync(join(nsisDir, exe), join("release", "Wisper-Setup.exe"));

const sigPath = join(nsisDir, `${exe}.sig`);
if (existsSync(sigPath)) {
  const signature = readFileSync(sigPath, "utf8").trim();
  const url = `https://github.com/${repo}/releases/download/v${version}/${exe}`;
  const entry = { signature, url };
  const latest = {
    version,
    notes: `Wisper ${version}`,
    pub_date: new Date().toISOString(),
    platforms: { "windows-x86_64": entry, "windows-x86_64-nsis": entry },
  };
  writeFileSync(join("release", "latest.json"), JSON.stringify(latest, null, 2));
  console.log("signed build: wrote latest.json");
} else {
  console.log("unsigned build (TAURI_SIGNING_PRIVATE_KEY secret not set): no latest.json, auto-update skipped");
}
console.log(readdirSync("release").join("\n"));
