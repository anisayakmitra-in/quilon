//! Compile-speed benchmark: how long each front-end phase takes on generated programs.
//!
//! Run it with `cargo bench`. There is no pass/fail here — it prints a table, and CI
//! publishes that table so the numbers can be watched over time. A regression shows up
//! as a column growing across commits, which is the thing nothing could see before.
//!
//! The corpora are generated rather than committed so their size is a number in one
//! place, and so they stay honest: each one stresses a different part of the pipeline
//! (sheer item count, expression depth, overload-set width, corelib imports).
//!
//! Phases are timed separately because they scale differently, and a total alone hides
//! which one moved. `link` resolves `<<` imports; it is zero for corpora with none.

use std::fmt::Write as _;
use std::time::{Duration, Instant};

use inkwell::context::Context;
use quilon::codegen::CodeGenerator;
use quilon::lexer::Lexer;
use quilon::parser;
use quilon::typechecker::TypeChecker;

/// How many times each corpus is compiled. The reported number is the mean; a handful
/// of runs is enough to damp scheduler noise without making the suite slow enough that
/// people skip it.
const RUNS: u32 = 5;

/// Where the corpora live, relative to the crate root. The files in here are the
/// benchmark's input: what runs is the committed bytes, so every run — yours, mine,
/// CI's, and one a year from now — compiles exactly the same programs.
const CORPUS_DIR: &str = "benches/corpus";

/// The corpora, in table order: file stem, and what the file is shaped to stress.
const CORPORA: &[(&str, &str)] = &[
    ("flat", "4000 top-level functions"),
    ("deep", "300 functions, each nested 100 deep"),
    ("wide_overloads", "300-member overload set"),
    ("corelib", "imports core.io/test/cli"),
];

fn main() {
    if std::env::args().any(|a| a == "--regen") {
        regenerate();
        return;
    }

    println!("Compile-speed benchmark — mean of {RUNS} runs, milliseconds\n");
    println!("| corpus | shape | bytes | lex | parse | link | check | codegen | total |");
    println!("|---|---|--:|--:|--:|--:|--:|--:|--:|");
    for (stem, shape) in CORPORA {
        let corpus = Corpus::read(stem, shape);
        let t = corpus.measure();
        println!(
            "| `{}` | {} | {} | {} | {} | {} | {} | {} | **{}** |",
            corpus.name,
            corpus.shape,
            corpus.source.len(),
            ms(t.lex),
            ms(t.parse),
            ms(t.link),
            ms(t.check),
            ms(t.codegen),
            ms(t.total()),
        );
    }
    // A trailing blank line closes the table for whatever reads it back out of the log.
    println!();
}

/// Rewrite `benches/corpus/` from the generators below — `cargo bench -- --regen`.
///
/// The generators are kept for this one purpose: producing a corpus of a different size
/// is a deliberate act whose result lands in git as a reviewable diff, not something
/// that happens quietly on the next run because a constant moved. Changing a corpus
/// breaks comparability with every number recorded before it, so it should be visible.
fn regenerate() {
    let dir = corpus_dir();
    for (stem, source) in [
        ("flat", flat_program(4000)),
        ("deep", deep_program(300, 100)),
        ("wide_overloads", overload_program(300)),
        ("corelib", corelib_program()),
    ] {
        let path = dir.join(format!("{stem}.ql"));
        std::fs::write(&path, source).unwrap_or_else(|e| panic!("writing {path:?}: {e}"));
        println!("wrote {}", path.display());
    }
}

fn corpus_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(CORPUS_DIR)
}

/// One corpus: the committed source, with a label for the table.
struct Corpus {
    name: &'static str,
    shape: &'static str,
    source: String,
}

#[derive(Default)]
struct Timing {
    lex: Duration,
    parse: Duration,
    link: Duration,
    check: Duration,
    codegen: Duration,
}

impl Timing {
    fn total(&self) -> Duration {
        self.lex + self.parse + self.link + self.check + self.codegen
    }
}

impl Corpus {
    /// Read a committed corpus. A missing file means someone deleted an input rather
    /// than that the benchmark should quietly measure something else, so it is fatal.
    fn read(name: &'static str, shape: &'static str) -> Self {
        let path = corpus_dir().join(format!("{name}.ql"));
        let source = std::fs::read_to_string(&path).unwrap_or_else(|e| {
            panic!("reading corpus {path:?}: {e} — run `cargo bench -- --regen` to rebuild it")
        });
        Self {
            name,
            shape,
            source,
        }
    }

    /// Compile the corpus `RUNS` times, accumulating each phase, and return the means.
    /// Every phase feeds the next exactly as the real driver wires them, so what is
    /// measured is the work a `quilon build` actually does — including handing codegen
    /// the table the checker just produced, rather than making it re-derive one.
    fn measure(&self) -> Timing {
        let mut total = Timing::default();
        for _ in 0..RUNS {
            let start = Instant::now();
            let tokens = Lexer::tokenize(&self.source).expect("benchmark corpus must lex");
            total.lex += start.elapsed();

            let start = Instant::now();
            let program = parser::parse(&tokens).expect("benchmark corpus must parse");
            total.parse += start.elapsed();

            let start = Instant::now();
            let program = quilon::modules::link(program, std::path::Path::new("."))
                .expect("benchmark corpus must resolve its imports");
            total.link += start.elapsed();

            let start = Instant::now();
            let table = TypeChecker::new()
                .check_program(&program)
                .expect("benchmark corpus must type-check");
            total.check += start.elapsed();

            let start = Instant::now();
            let context = Context::create();
            let mut codegen = CodeGenerator::new(&context, "bench");
            codegen.set_type_table(table);
            codegen
                .generate(&program)
                .expect("benchmark corpus must compile");
            total.codegen += start.elapsed();
        }
        Timing {
            lex: total.lex / RUNS,
            parse: total.parse / RUNS,
            link: total.link / RUNS,
            check: total.check / RUNS,
            codegen: total.codegen / RUNS,
        }
    }
}

fn ms(d: Duration) -> String {
    format!("{:.1}", d.as_secs_f64() * 1000.0)
}

/// Many small top-level functions: scales item count, so it moves with anything that is
/// per-declaration (registration, symbol mangling, emitting a function).
fn flat_program(count: usize) -> String {
    let mut src = String::new();
    for i in 0..count {
        let _ = writeln!(src, "f{i} = (x :: Num) -> Num => x * {i} + 1");
    }
    let _ = writeln!(src, "^ = () -> Num => f0(1) + f{}(2)", count - 1);
    src
}

/// Deeply parenthesized expressions: scales recursion depth rather than item count, so
/// it moves with anything per-level in the descent (the parser's precedence chain, the
/// checker's walk, expression lowering). Depth stays under the parser's nesting ceiling,
/// so the corpus gets its size from repeating the expression rather than nesting further.
fn deep_program(functions: usize, depth: usize) -> String {
    let mut expr = String::from("1");
    for i in 0..depth {
        expr = format!("({expr} + {i})");
    }
    let mut src = String::new();
    for i in 0..functions {
        let _ = writeln!(src, "d{i} = () -> Num => {expr}");
    }
    let _ = writeln!(src, "^ = () -> Num => d0()");
    src
}

/// One name with many members, and a call to each: scales overload-set width, so it
/// moves with resolution cost — which is a scan per call site.
fn overload_program(members: usize) -> String {
    let mut src = String::new();
    for i in 0..members {
        let _ = writeln!(src, "T{i} = {{ v :: Num }}");
    }
    for i in 0..members {
        let _ = writeln!(src, "pick = (a :: T{i}) -> Num => a.v");
    }
    let _ = writeln!(src, "^ = () -> Num => <");
    for i in 0..members {
        let _ = writeln!(src, "  n{i} = pick(T{i} {{ v = {i} }})");
    }
    let _ = writeln!(src, "  n0");
    let _ = writeln!(src, ">");
    src
}

/// A tiny program that imports the core library: almost all of its cost is the corelib
/// itself, which is checked and emitted whole whether or not the program uses it.
fn corelib_program() -> String {
    "<< core.io\n<< core.test\n<< core.cli\n\n^ = () -> $ => assert(1 + 1 == 2)\n".to_string()
}
