# Changelog

All notable changes to the Quilon VS Code extension are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.9.1] - 2026-08-10

Initial release. Version matches the Quilon compiler it targets.

### Added

- **Syntax highlighting** for Quilon (`.ql`) source files, driven by a
  TextMate grammar covering the language's symbol-based syntax (entry points,
  pattern matching, pipelines, comments, records, and sum types).
- **Inline diagnostics** — on open and on save of a `.ql` file, the extension
  runs `quilon check` on it, parses the compiler's `path:line:col: error:`
  output, and surfaces each error as an in-editor squiggle. Diagnostics update
  as you save and are cleared when a file checks clean.
- **Run / Check CodeLens** — a "▶ Run" and a "Check" action appear above every
  top-level `^` entry-point definition, invoking the compiler on the current
  file in an integrated terminal.
- **Commands** — "Quilon: Check Current File" and "Quilon: Run Current File",
  available from the Command Palette.
- **Configurable compiler invocation** via the `quilon.command` setting
  (defaults to `quilon` on your `PATH`; set it to e.g. `cargo run --` to drive
  the compiler from a checkout).
