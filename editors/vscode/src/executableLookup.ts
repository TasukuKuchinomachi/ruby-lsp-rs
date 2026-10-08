import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";

export function isExecutableFile(filePath: string): boolean {
  try {
    if (!fs.statSync(filePath).isFile()) {
      return false;
    }
    fs.accessSync(filePath, fs.constants.X_OK);
    return true;
  } catch {
    return false;
  }
}

export function executableFileName(name: string): string {
  return process.platform === "win32" ? `${name}.exe` : name;
}

export function findInPath(name: string): string | undefined {
  const directories = (process.env.PATH ?? "").split(path.delimiter).filter((directory) => directory !== "");
  for (const directory of directories) {
    const candidate = path.join(directory, executableFileName(name));
    if (isExecutableFile(candidate)) {
      return candidate;
    }
  }
  return undefined;
}

export function expandHomeDirectory(filePath: string): string {
  if (filePath === "~" || filePath.startsWith(`~${path.sep}`) || filePath.startsWith("~/")) {
    return path.join(os.homedir(), filePath.slice(1));
  }
  return filePath;
}
