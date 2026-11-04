import * as vscode from "vscode";
import { readRubyInlayHintsEnabled } from "./inlayHints";

export type ServerStatus = "stopped" | "starting" | "ready" | "error";

const statusPresentations: Record<ServerStatus, { icon: string; description: string }> = {
  stopped: { icon: "$(circle-slash)", description: "Server stopped" },
  starting: { icon: "$(sync~spin)", description: "Server starting" },
  ready: { icon: "$(ruby)", description: "Server ready" },
  error: { icon: "$(error)", description: "Server error" },
};

export class ServerStatusBar implements vscode.Disposable {
  private readonly statusBarItem: vscode.StatusBarItem;
  private status: ServerStatus = "stopped";
  private detail: string | undefined;

  constructor() {
    this.statusBarItem = vscode.window.createStatusBarItem("rubyLspRs.status", vscode.StatusBarAlignment.Left);
    this.statusBarItem.name = "Ruby LSP RS";
    this.statusBarItem.command = "rubyLspRs.toggleInlayHints";
    this.render();
    this.statusBarItem.show();
  }

  setStatus(status: ServerStatus, detail?: string): void {
    this.status = status;
    this.detail = detail;
    this.render();
  }

  render(): void {
    const presentation = statusPresentations[this.status];
    const inlayHintsEnabled = readRubyInlayHintsEnabled();
    this.statusBarItem.text = `${presentation.icon} Ruby LSP RS`;
    this.statusBarItem.backgroundColor =
      this.status === "error" ? new vscode.ThemeColor("statusBarItem.errorBackground") : undefined;

    const tooltip = new vscode.MarkdownString(undefined, true);
    tooltip.isTrusted = { enabledCommands: ["rubyLspRs.restartServer", "rubyLspRs.showOutput", "rubyLspRs.toggleInlayHints", "rubyLspRs.toggleTypeHintKind"] };
    tooltip.appendMarkdown(`**${presentation.description}**`);
    if (this.detail) {
      tooltip.appendMarkdown("\n\n");
      tooltip.appendText(this.detail);
    }
    tooltip.appendMarkdown(`\n\nInlay hints for Ruby: \`${inlayHintsEnabled}\` (click to toggle)`);
    tooltip.appendMarkdown(
      "\n\n---\n\n" +
        "[$(debug-restart) Restart Server](command:rubyLspRs.restartServer) · " +
        "[$(output) Show Output](command:rubyLspRs.showOutput) · " +
        "[$(list-selection) Type Hint Kinds](command:rubyLspRs.toggleTypeHintKind)",
    );
    this.statusBarItem.tooltip = tooltip;
  }

  dispose(): void {
    this.statusBarItem.dispose();
  }
}
