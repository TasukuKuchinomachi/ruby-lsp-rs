import * as path from "node:path";
import * as vscode from "vscode";
import { getConfiguration } from "./configuration";
import { executableFileName, expandHomeDirectory, findInPath, isExecutableFile } from "./executableLookup";

export const serverExecutableName = "ruby-lsp-rs";
export const serverPathEnvironmentVariable = "RUBY_LSP_RS_SERVER_PATH";

export interface ServerExecutableResolution {
  path: string | undefined;
  searchedLocations: string[];
}

function resolveAgainstWorkspace(filePath: string): string {
  const expandedPath = expandHomeDirectory(filePath);
  if (path.isAbsolute(expandedPath)) {
    return expandedPath;
  }
  const workspaceFolder = vscode.workspace.workspaceFolders?.[0];
  return workspaceFolder ? path.join(workspaceFolder.uri.fsPath, expandedPath) : path.resolve(expandedPath);
}

export function resolveServerExecutable(extensionPath: string): ServerExecutableResolution {
  const searchedLocations: string[] = [];

  const configuredPath = getConfiguration().get<string>("server.path", "").trim();
  if (configuredPath !== "") {
    const resolvedPath = resolveAgainstWorkspace(configuredPath);
    searchedLocations.push(`rubyLspRs.server.path: ${resolvedPath}`);
    return { path: isExecutableFile(resolvedPath) ? resolvedPath : undefined, searchedLocations };
  }

  const environmentPath = process.env[serverPathEnvironmentVariable]?.trim();
  if (environmentPath) {
    const resolvedPath = resolveAgainstWorkspace(environmentPath);
    searchedLocations.push(`${serverPathEnvironmentVariable}: ${resolvedPath}`);
    if (isExecutableFile(resolvedPath)) {
      return { path: resolvedPath, searchedLocations };
    }
  }

  const bundledPath = path.join(extensionPath, "server", executableFileName(serverExecutableName));
  searchedLocations.push(`bundled: ${bundledPath}`);
  if (isExecutableFile(bundledPath)) {
    return { path: bundledPath, searchedLocations };
  }

  searchedLocations.push(`PATH: ${serverExecutableName}`);
  return { path: findInPath(serverExecutableName), searchedLocations };
}
