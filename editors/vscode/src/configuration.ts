import * as vscode from "vscode";

export const configurationSection = "rubyLspRs";

export type DefinitionTarget = "implementation" | "signature" | "both";

export interface TypeCheckerSettings {
  enabled: boolean;
  diagnostics: { enabled: boolean };
  definition: { target: DefinitionTarget };
  inlayHints: {
    methodReturnTypes: boolean;
    attributeTypes: boolean;
    variableTypes: boolean;
    blockParameterTypes: boolean;
    methodParameterTypes: boolean;
    hideObviousTypes: boolean;
    hideUntyped: boolean;
    maxLength: number | null;
  };
}

export interface InitializationOptions {
  rubyCommand?: string[];
  enabledFeatures: Record<string, boolean>;
  featuresConfiguration: {
    inlayHint: { implicitRescue: boolean; implicitHashValue: boolean };
  };
  formatter: string;
  linters: string[] | null;
  typeChecker: TypeCheckerSettings;
}

export interface TypeHintKind {
  settingKey: string;
  label: string;
  description: string;
}

export const typeHintKinds: TypeHintKind[] = [
  {
    settingKey: "inlayHints.methodReturnTypes.enable",
    label: "Method return types",
    description: "Return types of def",
  },
  {
    settingKey: "inlayHints.methodParameterTypes.enable",
    label: "Method parameter types",
    description: "Parameters of def",
  },
  {
    settingKey: "inlayHints.attributeTypes.enable",
    label: "Attribute types",
    description: "attr_reader / attr_writer / attr_accessor",
  },
  {
    settingKey: "inlayHints.variableTypes.enable",
    label: "Variable types",
    description: "Variable assignments",
  },
  {
    settingKey: "inlayHints.blockParameterTypes.enable",
    label: "Block parameter types",
    description: "Parameters of blocks",
  },
];

export const settingsRequiringRestart = [
  "server.path",
  "server.extraEnv",
  "rubyCommand",
  "enabledFeatures",
  "formatter",
  "linters",
  "inlayHints.implicitRescue.enable",
  "inlayHints.implicitHashValue.enable",
];

export const settingsSentOnChange = [
  "typeChecker",
  "definition.target",
  ...typeHintKinds.map((typeHintKind) => typeHintKind.settingKey),
  "inlayHints.hideObviousTypes",
  "inlayHints.hideUntyped",
  "inlayHints.maxLength",
];

export function getConfiguration(): vscode.WorkspaceConfiguration {
  return vscode.workspace.getConfiguration(configurationSection);
}

export function readTypeCheckerSettings(): TypeCheckerSettings {
  const configuration = getConfiguration();
  return {
    enabled: configuration.get<boolean>("typeChecker.enable", true),
    diagnostics: {
      enabled: configuration.get<boolean>("typeChecker.diagnostics.enable", true),
    },
    definition: {
      target: configuration.get<DefinitionTarget>("definition.target", "implementation"),
    },
    inlayHints: {
      methodReturnTypes: configuration.get<boolean>("inlayHints.methodReturnTypes.enable", true),
      attributeTypes: configuration.get<boolean>("inlayHints.attributeTypes.enable", true),
      variableTypes: configuration.get<boolean>("inlayHints.variableTypes.enable", true),
      blockParameterTypes: configuration.get<boolean>("inlayHints.blockParameterTypes.enable", true),
      methodParameterTypes: configuration.get<boolean>("inlayHints.methodParameterTypes.enable", true),
      hideObviousTypes: configuration.get<boolean>("inlayHints.hideObviousTypes", true),
      hideUntyped: configuration.get<boolean>("inlayHints.hideUntyped", true),
      maxLength: configuration.get<number | null>("inlayHints.maxLength", 30),
    },
  };
}

export function readConfiguredRubyCommand(): string[] {
  return getConfiguration().get<string[]>("rubyCommand", []);
}

export function buildInitializationOptions(rubyCommand: string[] | undefined): InitializationOptions {
  const configuration = getConfiguration();
  const initializationOptions: InitializationOptions = {
    enabledFeatures: configuration.get<Record<string, boolean>>("enabledFeatures", {}),
    featuresConfiguration: {
      inlayHint: {
        implicitRescue: configuration.get<boolean>("inlayHints.implicitRescue.enable", false),
        implicitHashValue: configuration.get<boolean>("inlayHints.implicitHashValue.enable", false),
      },
    },
    formatter: configuration.get<string>("formatter", "auto"),
    linters: configuration.get<string[] | null>("linters", null),
    typeChecker: readTypeCheckerSettings(),
  };
  if (rubyCommand !== undefined) {
    initializationOptions.rubyCommand = rubyCommand;
  }
  return initializationOptions;
}

export function readExtraEnvironment(): Record<string, string> {
  const extraEnvironment = getConfiguration().get<Record<string, string | number>>("server.extraEnv", {});
  return Object.fromEntries(
    Object.entries(extraEnvironment).map(([name, value]) => [name, String(value)]),
  );
}

export async function updateSetting(section: string, key: string, value: unknown, overrideInLanguage = false): Promise<void> {
  const configuration = overrideInLanguage
    ? vscode.workspace.getConfiguration(section, { languageId: "ruby" })
    : vscode.workspace.getConfiguration(section);
  const inspection = configuration.inspect(key);
  const definedInWorkspace = overrideInLanguage
    ? inspection?.workspaceLanguageValue !== undefined
    : inspection?.workspaceValue !== undefined;
  const target = definedInWorkspace ? vscode.ConfigurationTarget.Workspace : vscode.ConfigurationTarget.Global;
  await configuration.update(key, value, target, overrideInLanguage);
}
