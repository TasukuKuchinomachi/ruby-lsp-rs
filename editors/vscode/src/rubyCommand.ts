import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import { executableFileName, findInPath, isExecutableFile } from "./executableLookup";

interface VersionManager {
  name: string;
  markerFiles: string[];
  commandArguments: string[];
  fallbackDirectories: string[];
}

const homeDirectory = os.homedir();

const versionManagers: VersionManager[] = [
  {
    name: "mise",
    markerFiles: [".tool-versions", "mise.toml", ".mise.toml"],
    commandArguments: ["exec", "--", "ruby"],
    fallbackDirectories: [
      path.join(homeDirectory, ".local", "bin"),
      path.join(homeDirectory, ".local", "share", "mise", "bin"),
      "/opt/homebrew/bin",
      "/usr/local/bin",
    ],
  },
  {
    name: "rbenv",
    markerFiles: [".ruby-version"],
    commandArguments: ["exec", "ruby"],
    fallbackDirectories: [path.join(homeDirectory, ".rbenv", "bin"), "/opt/homebrew/bin", "/usr/local/bin"],
  },
  {
    name: "asdf",
    markerFiles: [".tool-versions"],
    commandArguments: ["exec", "ruby"],
    fallbackDirectories: [path.join(homeDirectory, ".asdf", "bin"), "/opt/homebrew/bin", "/usr/local/bin"],
  },
];

export interface DetectedRubyCommand {
  command: string[];
  reason: string;
}

// VS Code launched from the Dock or Finder does not inherit the login shell PATH, so well-known install directories are checked as well.
function locateVersionManager(versionManager: VersionManager): string | undefined {
  if (findInPath(versionManager.name)) {
    return versionManager.name;
  }
  return versionManager.fallbackDirectories
    .map((directory) => path.join(directory, executableFileName(versionManager.name)))
    .find(isExecutableFile);
}

export function detectRubyCommand(workspaceDirectory: string | undefined): DetectedRubyCommand | undefined {
  if (workspaceDirectory === undefined) {
    return undefined;
  }
  for (const versionManager of versionManagers) {
    const markerFile = versionManager.markerFiles.find((fileName) =>
      fs.existsSync(path.join(workspaceDirectory, fileName)),
    );
    if (markerFile === undefined) {
      continue;
    }
    const executable = locateVersionManager(versionManager);
    if (executable === undefined) {
      continue;
    }
    return {
      command: [executable, ...versionManager.commandArguments],
      reason: `${markerFile} found in ${workspaceDirectory} and ${versionManager.name} is installed`,
    };
  }
  return undefined;
}
