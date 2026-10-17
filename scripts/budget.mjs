// Fails if shipped artifacts exceed budgets from AGENTS.md. Run after `pnpm build` (and after `tauri build` for the binary).
import { readdirSync, readFileSync, statSync, existsSync } from "node:fs";
import { join } from "node:path";
import { gzipSync } from "node:zlib";

const KB = 1024, MB = KB * KB;
const budgets = { js: 45 * KB, css: 10 * KB, bin: 12 * MB };
const files = (d) => readdirSync(d, { recursive: true }).map((f) => join(d, f)).filter((f) => statSync(f).isFile());
const gz = (ext) => files("dist").filter((f) => f.endsWith(ext)).reduce((n, f) => n + gzipSync(readFileSync(f), { level: 9 }).length, 0);

const results = [["js (gzip)", gz(".js"), budgets.js], ["css (gzip)", gz(".css"), budgets.css]];
const bin = ["src-tauri/target/release/kite.exe", "src-tauri/target/release/kite"].find(existsSync);
if (bin) results.push(["binary", statSync(bin).size, budgets.bin]);

let fail = false;
for (const [name, size, max] of results) {
  const ok = size <= max;
  fail ||= !ok;
  console.log(`${ok ? "ok  " : "FAIL"} ${name.padEnd(11)} ${(size / KB).toFixed(1).padStart(8)} KB / ${(max / KB).toFixed(0)} KB`);
}
if (!bin) console.log("skip binary      (no release build found)");
process.exit(fail ? 1 : 0);
