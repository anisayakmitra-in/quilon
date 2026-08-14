// Quilon debug integration.
//
// Debugging is delegated to CodeLLDB (`vadimcn.vscode-lldb`, a declared
// extension dependency). We contribute a `quilon` debug type whose
// DebugConfigurationProvider, when a session starts, builds the active `.ql`
// with `<command> build --debug <file> -o <tmpbin>` and then resolves into a
// CodeLLDB (`type: "lldb"`) launch of that binary. Breakpoints set in the `.ql`
// source are hit through the DWARF line table the `--debug` build emits.
//
// Richer *value* rendering (Text as a string, arrays as lists, records/sum
// variants) needs the distinct DWARF types the compiler's debug-types work will
// emit; that is not merged yet. The formatter file we import is scaffolded for
// it — see `formatters/quilon.py`.

import { execFile } from "node:child_process";
import * as fs from "node:fs";
import * as path from "node:path";
import * as vscode from "vscode";
import {
  buildArgs,
  firstNonEmptyLine,
  splitCommand,
  tempBinaryPath,
  toLldbConfiguration,
} from "./debugConfig";
import { quilonCommand } from "./extension";

/** The CodeLLDB extension we delegate to; also declared in extensionDependencies. */
const CODELLDB_ID = "vadimcn.vscode-lldb";

/**
 * Path to the shipped lldb formatter, or undefined if it isn't present (e.g. a
 * stripped package). A missing formatter must not break a debug session — it
 * only costs the pretty value rendering.
 */
function formatterPath(context: vscode.ExtensionContext): string | undefined {
  const p = path.join(context.extensionPath, "formatters", "quilon.py");
  return fs.existsSync(p) ? p : undefined;
}

/**
 * Resolve the `.ql` source to debug from the launch config and the active
 * editor. A configured `program` wins (variables are already substituted by the
 * time this runs); otherwise fall back to the active `.ql` editor so a bare F5
 * or the Debug CodeLens works without a launch.json.
 */
function resolveSourceFile(config: vscode.DebugConfiguration): string | undefined {
  const fromConfig = typeof config.program === "string" ? config.program.trim() : "";
  if (fromConfig.length > 0) {
    return fromConfig;
  }
  const active = vscode.window.activeTextEditor?.document;
  if (active && active.languageId === "quilon" && active.uri.scheme === "file") {
    return active.uri.fsPath;
  }
  return undefined;
}

/** Build `file` into `output` with DWARF line info; reject with the compiler's message on failure. */
function buildDebugBinary(file: string, output: string, cwd: string | undefined): Promise<void> {
  const { exe, baseArgs } = splitCommand(quilonCommand());
  return new Promise((resolve, reject) => {
    execFile(exe, buildArgs(baseArgs, file, output), { cwd }, (error, _stdout, stderr) => {
      if (error) {
        if ((error as NodeJS.ErrnoException).code === "ENOENT") {
          reject(
            new Error(
              `could not run "${exe}". Set "quilon.command" to your compiler (e.g. "cargo run --").`,
            ),
          );
          return;
        }
        reject(new Error(firstNonEmptyLine(stderr) ?? error.message));
        return;
      }
      resolve();
    });
  });
}

/**
 * Turns a `type: "quilon"` launch into a CodeLLDB launch: it builds the source
 * with debug info, then returns the equivalent `type: "lldb"` configuration for
 * VS Code to run. Returning `undefined` aborts the session (after surfacing why).
 */
class QuilonDebugConfigurationProvider implements vscode.DebugConfigurationProvider {
  constructor(private readonly context: vscode.ExtensionContext) {}

  provideDebugConfigurations(): vscode.DebugConfiguration[] {
    return [defaultDebugConfiguration()];
  }

  async resolveDebugConfigurationWithSubstitutedVariables(
    folder: vscode.WorkspaceFolder | undefined,
    config: vscode.DebugConfiguration,
  ): Promise<vscode.DebugConfiguration | undefined> {
    // A bare F5 with no launch.json hands us an empty config; adopt the default
    // identity but leave `program` unset so the active-editor fallback below runs.
    if (!config.type && !config.request && !config.name) {
      const { type, request, name } = defaultDebugConfiguration();
      config = { ...config, type, request, name };
    }

    if (!vscode.extensions.getExtension(CODELLDB_ID)) {
      void vscode.window.showErrorMessage(
        "Quilon debugging needs the CodeLLDB extension (vadimcn.vscode-lldb). Install it and try again.",
      );
      return undefined;
    }

    const file = resolveSourceFile(config);
    if (!file || !file.endsWith(".ql")) {
      void vscode.window.showErrorMessage("Quilon: no active .ql file to debug.");
      return undefined;
    }

    // The compiler reads from disk, so flush any dirty buffer for this file so
    // breakpoints line up with the built binary.
    const open = vscode.workspace.textDocuments.find((d) => d.uri.fsPath === file);
    if (open?.isDirty) {
      await open.save();
    }

    const cwd =
      folder?.uri.fsPath ?? vscode.workspace.getWorkspaceFolder(vscode.Uri.file(file))?.uri.fsPath;
    const output = tempBinaryPath(file);

    try {
      await buildDebugBinary(file, output, cwd);
    } catch (error) {
      void vscode.window.showErrorMessage(
        `Quilon: debug build failed: ${(error as Error).message}`,
      );
      return undefined;
    }

    const programArgs = Array.isArray(config.args) ? (config.args as string[]) : [];
    return toLldbConfiguration({
      name: typeof config.name === "string" ? config.name : "Quilon Debug",
      program: output,
      args: programArgs,
      cwd,
      formatterPath: formatterPath(this.context),
    }) as vscode.DebugConfiguration;
  }
}

/** The default launch config we contribute and start from the Debug CodeLens. */
function defaultDebugConfiguration(): vscode.DebugConfiguration {
  return {
    type: "quilon",
    request: "launch",
    name: "Quilon: Debug current file",
    program: "${file}",
    args: [],
  };
}

/** Register the debug provider and the `quilon.debug` command (used by the CodeLens). */
export function registerDebug(context: vscode.ExtensionContext): void {
  context.subscriptions.push(
    vscode.debug.registerDebugConfigurationProvider(
      "quilon",
      new QuilonDebugConfigurationProvider(context),
    ),
    vscode.commands.registerCommand("quilon.debug", () => {
      // `${file}` resolves to the active editor; the provider owns validation
      // and the "no active .ql file" error, so this stays a thin trigger.
      const doc = vscode.window.activeTextEditor?.document;
      const folder = doc ? vscode.workspace.getWorkspaceFolder(doc.uri) : undefined;
      void vscode.debug.startDebugging(folder, defaultDebugConfiguration());
    }),
  );
}
