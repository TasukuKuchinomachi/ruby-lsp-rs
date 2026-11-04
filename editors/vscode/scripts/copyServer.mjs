import * as fs from "node:fs";
import * as path from "node:path";
import { fileURLToPath } from "node:url";

const extensionDirectory = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const executableName = process.platform === "win32" ? "ruby-lsp-rs.exe" : "ruby-lsp-rs";
const sourcePath = path.resolve(extensionDirectory, "..", "..", "target", "release", executableName);
const destinationDirectory = path.join(extensionDirectory, "server");
const destinationPath = path.join(destinationDirectory, executableName);

fs.rmSync(destinationDirectory, { recursive: true, force: true });

if (!fs.existsSync(sourcePath)) {
  console.warn(`${sourcePath} not found; packaging without a bundled server. Run \`cargo build --release\` to bundle it.`);
  process.exit(0);
}

fs.mkdirSync(destinationDirectory, { recursive: true });
fs.copyFileSync(sourcePath, destinationPath);
fs.chmodSync(destinationPath, 0o755);
console.log(`Bundled ${sourcePath} -> ${destinationPath}`);
