//! Shared compiler front-end.
//!
//! The `check`, `compile`, and `run` commands all begin the same way: read the
//! source file, lex it, parse it, resolve its `<<` imports, and type-check the
//! result. This module owns that pipeline so the commands only differ in their
//! tails (print a summary, emit LLVM IR, or JIT-execute).

use std::path::Path;

use crate::diagnostic::{self, Severity};
use crate::lexer::Span;
use crate::{ast, lexer, modules, parser, typechecker};

/// A failure from any stage of the front-end. Its `Display` is the exact
/// diagnostic the CLI prints to stderr before exiting: for stages that know a
/// source location (`lex`, `parse`, `type`) it is a rustc-style
/// `path:line:col: error: …` report with the offending source line and a caret;
/// for location-less failures (`read`, `import`) it is a one-line message.
pub struct FrontEndError {
    /// The diagnostic, fully rendered against the source at construction time.
    rendered: String,
}

impl std::fmt::Display for FrontEndError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.rendered)
    }
}

impl FrontEndError {
    /// A source-located error: render it rustc-style with the caret context.
    fn at(path: &str, source: &str, span: &Span, message: &str) -> Self {
        Self {
            rendered: diagnostic::render(path, source, span, Severity::Error, message),
        }
    }

    /// An error with no source location (file read failure, import resolution).
    fn plain(message: String) -> Self {
        Self { rendered: message }
    }
}

/// A program that has passed the front end, together with what the later stages need
/// from that pass.
///
/// `types` is the whole point of returning a struct: the type checker computes an
/// inferred type for every expression, and codegen needs them to lower reads at their
/// declared type. Recomputing that table means type-checking the program a second time,
/// which is what this carries it here to avoid.
pub struct Checked {
    /// The import-linked, type-checked program.
    pub program: ast::Program,
    /// Every expression's inferred type, keyed by source position.
    pub types: typechecker::TypeTable,
    /// The source text of `file`, for mapping a span back to a line and column.
    pub source: String,
    /// How many leading items came from `<<` imports — `link` prepends them, so anything
    /// before this index belongs to another file. A `--debug` build uses it to attribute
    /// DWARF line info to the user's own source only.
    pub imported_items: usize,
    /// The deferred-value coloring: which expressions evaluate to a deferred (promise)
    /// value, and whether any `@` primitive launch is reachable. Codegen reads it to emit
    /// the promise representation and forces; empty for pure programs.
    pub defer: crate::deferral::DeferInfo,
}

/// Read, lex, parse, resolve `<<` imports (relative to `file`'s directory), and
/// type-check the program at `file`.
pub fn front_end(file: &Path) -> Result<Checked, FrontEndError> {
    let path = file.display().to_string();

    let source = std::fs::read_to_string(file)
        .map_err(|e| FrontEndError::plain(format!("error reading {}: {}", path, e)))?;

    let tokens = lexer::Lexer::tokenize(&source)
        .map_err(|e| FrontEndError::at(&path, &source, &e.span, &e.message))?;

    let program = parser::parse(&tokens)
        .map_err(|e| FrontEndError::at(&path, &source, &e.span, &e.message))?;

    // The `@` marker names a leaf IO primitive, which only the corelib/runtime may
    // define; user code merely *calls* one. Reject an `@`-prefixed declaration in the
    // program's own source with a source-located diagnostic (a bare parse error would be
    // cryptic). Checked before `link` so only the user's items are scanned, never a
    // built-in module's.
    if let Some((span, name)) = first_at_declaration(&program) {
        return Err(FrontEndError::at(
            &path,
            &source,
            span,
            &format!(
                "`{name}` cannot be declared here: `@` marks a built-in IO primitive \
                 (like `@sleep` from core.time), which only the corelib defines — user \
                 code calls one, it does not declare one"
            ),
        ));
    }

    // The source file's own item count, captured before linking prepends imported items.
    let own_item_count = program.items.len();
    let base_dir = file.parent().unwrap_or_else(|| Path::new("."));
    let program = modules::link(program, base_dir).map_err(FrontEndError::plain)?;
    // `link` prepends imported items, so everything before the source's own items is imported.
    let imported_items = program.items.len() - own_item_count;

    let types = typechecker::TypeChecker::new()
        .check_program(&program)
        .map_err(|e| FrontEndError::at(&path, &source, e.span(), &e.to_string()))?;

    // Deferred-value analysis (post-typecheck, pre-codegen): whether an `@` primitive is
    // reached, and the taint / force-set for value-returning primitives. Reads no types and
    // adds none, so the check above is unaffected. The `@read` launch sites need the source
    // to render `path:line:col`, which only lives here — so fill them in now.
    let mut defer = crate::deferral::analyze(&program);
    defer.set_read_sites(crate::deferral::read_call_sites(&program, &path, &source));

    Ok(Checked {
        program,
        types,
        source,
        imported_items,
        defer,
    })
}

/// The span and name of the first top-level declaration whose name starts with `@`, if
/// any. Used to reject a user-written `@` primitive declaration (they are corelib-only).
fn first_at_declaration(program: &ast::Program) -> Option<(&Span, &str)> {
    program.items.iter().find_map(|item| match item {
        ast::Item::FunctionDecl(d) if d.name.starts_with('@') => Some((&d.span, d.name.as_str())),
        ast::Item::VarDecl(d) if d.name.starts_with('@') => Some((&d.span, d.name.as_str())),
        _ => None,
    })
}

/// Whether `program` defines the `^` entry point required to build an executable.
pub fn has_entry_point(program: &ast::Program) -> bool {
    program
        .items
        .iter()
        .any(|item| matches!(item, ast::Item::FunctionDecl(func) if func.name == "^"))
}
