// Unit tests for the pure debug-config helpers. No `vscode` dependency, so
// these run under plain Node: `pnpm test` compiles to out/ then `node --test`.

import assert from "node:assert/strict";
import * as path from "node:path";
import { test } from "node:test";
import {
  buildArgs,
  firstNonEmptyLine,
  splitCommand,
  tempBinaryPath,
  toLldbConfiguration,
} from "./debugConfig";

test("firstNonEmptyLine: skips leading blank lines and trims", () => {
  assert.equal(firstNonEmptyLine("\n\n   error: boom  \nmore"), "error: boom");
});

test("firstNonEmptyLine: undefined when there is no content", () => {
  assert.equal(firstNonEmptyLine("\n   \n"), undefined);
});

test("splitCommand: bare `quilon` has no base args", () => {
  assert.deepEqual(splitCommand("quilon"), { exe: "quilon", baseArgs: [] });
});

test("splitCommand: `cargo run --` splits into exe + leading args", () => {
  assert.deepEqual(splitCommand("cargo run --"), { exe: "cargo", baseArgs: ["run", "--"] });
});

test("splitCommand: empty/whitespace falls back to quilon", () => {
  assert.deepEqual(splitCommand("   "), { exe: "quilon", baseArgs: [] });
});

test("buildArgs: emits a `build --debug <file> -o <out>` invocation", () => {
  assert.deepEqual(buildArgs([], "/w/app.ql", "/tmp/app"), [
    "build",
    "--debug",
    "/w/app.ql",
    "-o",
    "/tmp/app",
  ]);
});

test("buildArgs: preserves base args from the command setting", () => {
  assert.deepEqual(buildArgs(["run", "--"], "/w/app.ql", "/tmp/app"), [
    "run",
    "--",
    "build",
    "--debug",
    "/w/app.ql",
    "-o",
    "/tmp/app",
  ]);
});

test("tempBinaryPath: strips .ql, embeds base name + pid + uniquifier, honors tmpDir", () => {
  const out = tempBinaryPath("/some/where/factorial.ql", "abc", "/mytmp");
  assert.equal(out, path.join("/mytmp", `quilon-debug-factorial-${process.pid}-abc`));
});

test("tempBinaryPath: is case-insensitive on the .ql extension", () => {
  const out = tempBinaryPath("/x/APP.QL", "1", "/t");
  assert.equal(path.basename(out), `quilon-debug-APP-${process.pid}-1`);
});

test("toLldbConfiguration: resolves to a CodeLLDB launch of the built binary", () => {
  const config = toLldbConfiguration({
    name: "Quilon: Debug current file",
    program: "/tmp/app",
    args: ["a", "b"],
    cwd: "/w",
  });
  assert.equal(config.type, "lldb");
  assert.equal(config.request, "launch");
  assert.equal(config.program, "/tmp/app");
  assert.deepEqual(config.args, ["a", "b"]);
  assert.equal(config.cwd, "/w");
  assert.equal(config.initCommands, undefined);
});

test("toLldbConfiguration: imports the formatter when a path is given", () => {
  const config = toLldbConfiguration({
    name: "Quilon Debug",
    program: "/tmp/app",
    formatterPath: "/ext/formatters/quilon.py",
  });
  assert.deepEqual(config.initCommands, ['command script import "/ext/formatters/quilon.py"']);
});

test("toLldbConfiguration: defaults missing args and cwd", () => {
  const config = toLldbConfiguration({ name: "n", program: "/tmp/app" });
  assert.deepEqual(config.args, []);
  assert.equal(config.cwd, "${workspaceFolder}");
});
