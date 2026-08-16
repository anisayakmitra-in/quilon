//! The harness every integration test drives the compiler through.
//!
//! Each `tests/*.rs` file is its own binary, so this module is compiled into each one
//! separately — including `JIT_LOCK`, which therefore stays exactly what it was when
//! each file declared its own: a lock over the JIT within one test binary.
//!
//! That same per-binary compilation is why the whole module allows dead code: a binary
//! that only needs `assert_exit` still compiles `assert_type_error` and the linking
//! variants, and would warn about every helper it happens not to call. The allowance is
//! about how shared test modules build, not about keeping unused code around — every
//! helper here has callers, just never all of them in one binary.
#![allow(dead_code)]

use quilon::jit;
use quilon::lexer::Lexer;
use quilon::parser;
use quilon::typechecker::{TypeChecker, TypeTable};
use std::path::Path;
use std::sync::Mutex;

/// LLVM's JIT and native-target initialization are not safe to run from several threads
/// at once, and cargo runs a binary's tests in parallel — so every execution below is
/// serialized through this.
pub static JIT_LOCK: Mutex<()> = Mutex::new(());

/// Compile and run `src`, asserting the entry point yields `expected` as the exit code.
/// The front end must succeed: a program that fails to lex, parse, or type-check is a
/// broken test, not a passing one.
pub fn assert_exit(src: &str, expected: i32) {
    let _guard = JIT_LOCK.lock().unwrap_or_else(|p| p.into_inner());

    let (program, types) = front_end(src, None);
    let code =
        jit::run_program(&program, types, &["program".to_string()]).expect("execution failed");
    assert_eq!(code, expected, "unexpected exit code for source:\n{src}");
}

/// Like [`assert_exit`], but resolves `<<` imports first, so a program that uses the
/// core library (`<< core.io`, `<< core.test`, …) runs end to end.
pub fn assert_exit_linked(src: &str, expected: i32) {
    assert_exit_linked_from(src, Path::new("."), expected);
}

/// Like [`assert_exit_linked`], resolving file-path imports relative to `base_dir`.
pub fn assert_exit_linked_from(src: &str, base_dir: &Path, expected: i32) {
    let _guard = JIT_LOCK.lock().unwrap_or_else(|p| p.into_inner());

    let (program, types) = front_end(src, Some(base_dir));
    let code =
        jit::run_program(&program, types, &["program".to_string()]).expect("execution failed");
    assert_eq!(code, expected, "unexpected exit code for source:\n{src}");
}

/// Assert the type checker REJECTS `src`. Lexing and parsing must still succeed: the
/// point is that the checker caught it, so a source that dies earlier would pass this
/// for the wrong reason.
pub fn assert_type_error(src: &str) {
    let tokens = Lexer::tokenize(src).expect("lexing failed");
    let program = parser::parse(&tokens).expect("parsing failed");
    assert!(
        TypeChecker::new().check_program(&program).is_err(),
        "expected a type error for source:\n{src}"
    );
}

/// Lex, parse, optionally resolve imports, and type-check — panicking with the stage
/// that failed. Returns the program with the type table its check produced, which is
/// what codegen runs on. Shared by the run helpers above.
fn front_end(src: &str, base_dir: Option<&Path>) -> (quilon::ast::Program, TypeTable) {
    let tokens = Lexer::tokenize(src).expect("lexing failed");
    let program = parser::parse(&tokens).expect("parsing failed");
    let program = match base_dir {
        Some(dir) => quilon::modules::link(program, dir).expect("import linking failed"),
        None => program,
    };
    let types = TypeChecker::new()
        .check_program(&program)
        .expect("type checking failed");
    (program, types)
}
