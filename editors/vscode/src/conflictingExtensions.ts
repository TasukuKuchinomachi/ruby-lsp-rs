import * as vscode from "vscode";

const conflictingExtensions = [
  { identifier: "soutaro.steep-vscode", name: "Steep" },
  { identifier: "Shopify.ruby-lsp", name: "Ruby LSP" },
];

const dismissedStateKey = "conflictingExtensionsNotification.dismissed";

export async function notifyConflictingExtensions(context: vscode.ExtensionContext): Promise<void> {
  if (context.globalState.get<boolean>(dismissedStateKey, false)) {
    return;
  }
  const activeConflicts = conflictingExtensions.filter(
    (extension) => vscode.extensions.getExtension(extension.identifier) !== undefined,
  );
  if (activeConflicts.length === 0) {
    return;
  }

  const names = activeConflicts.map((extension) => `${extension.name} (${extension.identifier})`).join(", ");
  const showExtensionsAction = "Show Extensions";
  const dismissAction = "Don't Show Again";
  const selectedAction = await vscode.window.showInformationMessage(
    `${names} ${activeConflicts.length === 1 ? "is" : "are"} also enabled. Hover, diagnostics and inlay hints for Ruby may be duplicated. ` +
      `Consider disabling ${activeConflicts.length === 1 ? "it" : "them"} for this workspace while using Ruby LSP RS.`,
    showExtensionsAction,
    dismissAction,
  );
  if (selectedAction === showExtensionsAction) {
    await vscode.commands.executeCommand(
      "workbench.extensions.search",
      activeConflicts.map((extension) => `@id:${extension.identifier}`).join(" "),
    );
  } else if (selectedAction === dismissAction) {
    await context.globalState.update(dismissedStateKey, true);
  }
}
