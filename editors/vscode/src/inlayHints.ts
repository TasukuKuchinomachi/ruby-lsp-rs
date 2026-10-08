import * as vscode from "vscode";
import { configurationSection, getConfiguration, typeHintKinds, updateSetting } from "./configuration";

type InlayHintsEnabledValue = "on" | "off" | "onUnlessPressed" | "offUnlessPressed";

export function readRubyInlayHintsEnabled(): InlayHintsEnabledValue {
  return vscode.workspace
    .getConfiguration("editor", { languageId: "ruby" })
    .get<InlayHintsEnabledValue>("inlayHints.enabled", "on");
}

export function areRubyInlayHintsVisibleByDefault(): boolean {
  const value = readRubyInlayHintsEnabled();
  return value === "on" || value === "onUnlessPressed";
}

export async function toggleInlayHints(): Promise<void> {
  const nextValue: InlayHintsEnabledValue = areRubyInlayHintsVisibleByDefault() ? "off" : "on";
  await updateSetting("editor", "inlayHints.enabled", nextValue, true);
}

interface TypeHintKindItem extends vscode.QuickPickItem {
  settingKey: string;
}

export async function toggleTypeHintKind(): Promise<void> {
  const configuration = getConfiguration();
  const items: TypeHintKindItem[] = typeHintKinds.map((typeHintKind) => ({
    label: typeHintKind.label,
    description: typeHintKind.description,
    settingKey: typeHintKind.settingKey,
    picked: configuration.get<boolean>(typeHintKind.settingKey, true),
  }));

  const selectedItems = await vscode.window.showQuickPick(items, {
    canPickMany: true,
    title: "Ruby LSP RS: Type Hint Kinds",
    placeHolder: "Select the kinds of type hints to show",
  });
  if (selectedItems === undefined) {
    return;
  }

  const selectedSettingKeys = new Set(selectedItems.map((item) => item.settingKey));
  for (const item of items) {
    const enabled = selectedSettingKeys.has(item.settingKey);
    if (enabled !== item.picked) {
      await updateSetting(configurationSection, item.settingKey, enabled);
    }
  }
}
