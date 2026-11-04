import * as vscode from "vscode";
import {
  DidChangeConfigurationNotification,
  LanguageClient,
  LanguageClientOptions,
  RevealOutputChannelOn,
  ServerOptions,
  State,
} from "vscode-languageclient/node";
import {
  buildInitializationOptions,
  configurationSection,
  readConfiguredRubyCommand,
  readExtraEnvironment,
  readTypeCheckerSettings,
  settingsRequiringRestart,
  settingsSentOnChange,
} from "./configuration";
import { notifyConflictingExtensions } from "./conflictingExtensions";
import { toggleInlayHints, toggleTypeHintKind } from "./inlayHints";
import { detectRubyCommand } from "./rubyCommand";
import { resolveServerExecutable } from "./serverExecutable";
import { ServerStatusBar } from "./statusBar";

let languageClient: LanguageClient | undefined;
let outputChannel: vscode.OutputChannel;
let traceOutputChannel: vscode.OutputChannel;
let statusBar: ServerStatusBar;

export async function activate(context: vscode.ExtensionContext): Promise<void> {
  outputChannel = vscode.window.createOutputChannel("Ruby LSP RS");
  traceOutputChannel = vscode.window.createOutputChannel("Ruby LSP RS Trace");
  statusBar = new ServerStatusBar();
  context.subscriptions.push(outputChannel, traceOutputChannel, statusBar);

  context.subscriptions.push(
    vscode.commands.registerCommand("rubyLspRs.restartServer", () => restartServer(context)),
    vscode.commands.registerCommand("rubyLspRs.showOutput", () => outputChannel.show(true)),
    vscode.commands.registerCommand("rubyLspRs.toggleInlayHints", toggleInlayHints),
    vscode.commands.registerCommand("rubyLspRs.toggleTypeHintKind", toggleTypeHintKind),
    vscode.workspace.onDidChangeConfiguration((event) => handleConfigurationChange(event, context)),
  );

  void notifyConflictingExtensions(context);
  await startServer(context);
}

export async function deactivate(): Promise<void> {
  await stopServer();
}

function workspaceDirectory(): string | undefined {
  return vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
}

function resolveRubyCommand(): string[] | undefined {
  const configuredRubyCommand = readConfiguredRubyCommand();
  if (configuredRubyCommand.length > 0) {
    outputChannel.appendLine(`Using rubyCommand from settings: ${JSON.stringify(configuredRubyCommand)}`);
    return configuredRubyCommand;
  }
  const detectedRubyCommand = detectRubyCommand(workspaceDirectory());
  if (detectedRubyCommand === undefined) {
    outputChannel.appendLine("No Ruby version manager detected; the server will use ruby on PATH.");
    return undefined;
  }
  outputChannel.appendLine(
    `Detected rubyCommand ${JSON.stringify(detectedRubyCommand.command)} (${detectedRubyCommand.reason}).`,
  );
  return detectedRubyCommand.command;
}

async function startServer(context: vscode.ExtensionContext): Promise<void> {
  const serverExecutable = resolveServerExecutable(context.extensionPath);
  if (serverExecutable.path === undefined) {
    const message = "Could not find the ruby-lsp-rs server executable.";
    outputChannel.appendLine(`${message} Searched:\n  ${serverExecutable.searchedLocations.join("\n  ")}`);
    statusBar.setStatus("error", message);
    const openSettingsAction = "Open Settings";
    const showOutputAction = "Show Output";
    const selectedAction = await vscode.window.showErrorMessage(
      `${message} Build it with \`cargo build --release\` and set rubyLspRs.server.path, or put ruby-lsp-rs on PATH.`,
      openSettingsAction,
      showOutputAction,
    );
    if (selectedAction === openSettingsAction) {
      await vscode.commands.executeCommand("workbench.action.openSettings", `${configurationSection}.server.path`);
    } else if (selectedAction === showOutputAction) {
      outputChannel.show(true);
    }
    return;
  }

  outputChannel.appendLine(`Using server executable: ${serverExecutable.path}`);
  const serverOptions: ServerOptions = {
    command: serverExecutable.path,
    options: {
      cwd: workspaceDirectory(),
      env: { ...process.env, ...readExtraEnvironment() },
    },
  };

  const clientOptions: LanguageClientOptions = {
    documentSelector: [
      { scheme: "file", language: "ruby" },
      { scheme: "untitled", language: "ruby" },
      { scheme: "file", pattern: "**/*.rbs" },
    ],
    initializationOptions: buildInitializationOptions(resolveRubyCommand()),
    outputChannel,
    traceOutputChannel,
    revealOutputChannelOn: RevealOutputChannelOn.Never,
  };

  const client = new LanguageClient(configurationSection, "Ruby LSP RS", serverOptions, clientOptions);
  languageClient = client;
  client.onDidChangeState((event) => {
    if (languageClient !== client) {
      return;
    }
    if (event.newState === State.Starting) {
      statusBar.setStatus("starting");
    } else if (event.newState === State.Running) {
      statusBar.setStatus("ready");
    } else if (event.newState === State.Stopped && event.oldState === State.Running) {
      statusBar.setStatus("error", "The server stopped unexpectedly. See the output for details.");
    }
  });

  statusBar.setStatus("starting");
  try {
    await client.start();
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    outputChannel.appendLine(`Failed to start the server: ${message}`);
    statusBar.setStatus("error", message);
    void vscode.window.showErrorMessage(`Ruby LSP RS: failed to start the server: ${message}`);
  }
}

async function stopServer(): Promise<void> {
  const client = languageClient;
  languageClient = undefined;
  if (client === undefined) {
    return;
  }
  try {
    await client.dispose();
  } catch (error) {
    outputChannel.appendLine(`Failed to stop the server: ${error instanceof Error ? error.message : String(error)}`);
  }
  statusBar.setStatus("stopped");
}

async function restartServer(context: vscode.ExtensionContext): Promise<void> {
  await stopServer();
  await startServer(context);
}

function affectsAny(event: vscode.ConfigurationChangeEvent, keys: string[]): boolean {
  return keys.some((key) => event.affectsConfiguration(`${configurationSection}.${key}`));
}

async function handleConfigurationChange(
  event: vscode.ConfigurationChangeEvent,
  context: vscode.ExtensionContext,
): Promise<void> {
  if (event.affectsConfiguration("editor.inlayHints.enabled", { languageId: "ruby" })) {
    statusBar.render();
  }

  if (affectsAny(event, settingsSentOnChange) && languageClient?.isRunning()) {
    await languageClient.sendNotification(DidChangeConfigurationNotification.type, {
      settings: { typeChecker: readTypeCheckerSettings() },
    });
  }

  if (affectsAny(event, settingsRequiringRestart)) {
    const restartAction = "Restart Server";
    const selectedAction = await vscode.window.showInformationMessage(
      "Ruby LSP RS: the changed setting takes effect after restarting the server.",
      restartAction,
    );
    if (selectedAction === restartAction) {
      await restartServer(context);
    }
  }
}
