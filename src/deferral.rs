//! Deferral analysis — the compiler's view of Quilon's `@` leaf-IO-primitive tier.
//!
//! Today its one job is small and exact: decide whether a program reaches any `@` primitive
//! (`uses_deferral`), so codegen runs the entry on a scheduler fiber only then — a pure
//! program is compiled exactly as before, with zero overhead. It also rejects an `@` call in
//! a *top-level* binding, which would run before the scheduler starts (the primitive would
//! have no fiber to park on). It reads no types and adds none, so the type checker is
//! unaffected: an `@` primitive keeps its ordinary result type (`@sleep` is plain `$`).
//!
//! This is the growth point for the fuller strictness analysis (the deferred-value taint and
//! force-frontier) that arrives with a value-returning primitive (`@read`).

use crate::ast::{Expr, InterpPart, Item, Program, Statement};
use crate::lexer::Span;

/// What the analysis hands to codegen.
#[derive(Debug, Default, Clone)]
pub struct DeferInfo {
    /// Whether any `@` leaf IO primitive is reachable. Gates running the entry on a
    /// scheduler fiber: a program that uses no `@` primitive is byte-identical to before.
    pub uses_deferral: bool,
}

/// An `@` primitive used where it cannot run yet (a top-level binding, before the scheduler
/// starts). Carries the span so the front end can render a source-located diagnostic.
#[derive(Debug)]
pub struct DeferError {
    span: Span,
    message: String,
}

impl DeferError {
    pub fn span(&self) -> &Span {
        &self.span
    }
}

impl std::fmt::Display for DeferError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

/// Analyze `program`: report whether it uses an `@` primitive, or the first `@` call in a
/// position that cannot run yet.
pub fn analyze(program: &Program) -> Result<DeferInfo, DeferError> {
    let mut info = DeferInfo::default();
    for item in &program.items {
        match item {
            // A function body runs on the entry fiber (directly or via a call), so an `@`
            // primitive there is fine.
            Item::FunctionDecl(f) => {
                if first_at_reference(&f.body).is_some() {
                    info.uses_deferral = true;
                }
            }
            // A top-level binding's value is evaluated before the scheduler is running, so
            // an `@` primitive there has no fiber to park on. Reject it with a clear error.
            Item::VarDecl(v) => {
                if let Some(span) = first_at_reference(&v.value) {
                    return Err(DeferError {
                        span: span.clone(),
                        message: "an `@` primitive cannot be called in a top-level binding \
                                  (it runs before the concurrency runtime starts) — call it \
                                  inside a function body instead"
                            .to_string(),
                    });
                }
            }
            Item::TypeDecl(_) => {}
        }
    }
    Ok(info)
}

/// The span of the first `@`-primitive reference anywhere in `expr`, if any. An `@` name can
/// only ever name a leaf IO primitive (the parser reserves the `@`), so any `@`-prefixed
/// identifier — whether called directly, piped into, or bound — counts.
fn first_at_reference(expr: &Expr) -> Option<&Span> {
    match expr {
        Expr::Ident { name, span } if name.starts_with('@') => Some(span),
        Expr::Ident { .. }
        | Expr::Number { .. }
        | Expr::String { .. }
        | Expr::Bool { .. }
        | Expr::Unit { .. } => None,

        Expr::Interpolation { parts, .. } => parts.iter().find_map(|p| match p {
            InterpPart::Hole(e) => first_at_reference(e),
            InterpPart::Lit(_) => None,
        }),
        Expr::Call { func, args, .. } => {
            first_at_reference(func).or_else(|| args.iter().find_map(first_at_reference))
        }
        Expr::BinOp { left, right, .. } | Expr::Pipeline { left, right, .. } => {
            first_at_reference(left).or_else(|| first_at_reference(right))
        }
        Expr::Range { start, end, .. } => {
            first_at_reference(start).or_else(|| first_at_reference(end))
        }
        Expr::UnaryOp { expr, .. }
        | Expr::FieldAccess { expr, .. }
        | Expr::Spread { expr, .. }
        | Expr::Lambda { body: expr, .. } => first_at_reference(expr),
        Expr::FieldAssign { target, value, .. } => {
            first_at_reference(target).or_else(|| first_at_reference(value))
        }
        Expr::Index { expr, index, .. } => {
            first_at_reference(expr).or_else(|| first_at_reference(index))
        }
        Expr::If {
            cond, then, else_, ..
        } => first_at_reference(cond)
            .or_else(|| first_at_reference(then))
            .or_else(|| first_at_reference(else_)),
        Expr::Match { expr, arms, .. } => {
            first_at_reference(expr).or_else(|| arms.iter().find_map(|a| first_at_reference(&a.body)))
        }
        Expr::Array { elements, .. } => elements.iter().find_map(first_at_reference),
        Expr::Record { fields, .. } | Expr::Constructor { fields, .. } => {
            fields.iter().find_map(|(_, e)| first_at_reference(e))
        }
        Expr::Block { stmts, .. } => stmts.iter().find_map(|s| match s {
            Statement::Expr(e) => first_at_reference(e),
            Statement::Item(Item::VarDecl(v)) => first_at_reference(&v.value),
            Statement::Item(Item::FunctionDecl(f)) => first_at_reference(&f.body),
            Statement::Item(Item::TypeDecl(_)) => None,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::Lexer;
    use crate::parser;

    fn info(src: &str) -> Result<DeferInfo, DeferError> {
        let tokens = Lexer::tokenize(src).expect("lex");
        let program = parser::parse(&tokens).expect("parse");
        analyze(&program)
    }

    #[test]
    fn pure_program_uses_no_deferral() {
        assert!(!info("^ = () -> Num => 1 + 2 * 3").expect("ok").uses_deferral);
    }

    #[test]
    fn a_sleep_call_marks_deferral() {
        assert!(
            info("^ = () -> $ => <\n  @sleep(1)\n  $\n>")
                .expect("ok")
                .uses_deferral
        );
    }

    #[test]
    fn sleep_reached_through_a_helper_marks_deferral() {
        let src = "nap = () -> $ => @sleep(1)\n^ = () -> $ => nap()";
        assert!(info(src).expect("ok").uses_deferral);
    }

    #[test]
    fn sleep_in_a_top_level_binding_is_rejected() {
        // A global initializer runs before the scheduler, so `@sleep` there has no fiber.
        assert!(info("x = @sleep(1)\n^ = () -> Num => 0").is_err());
    }

    #[test]
    fn sleep_piped_in_still_marks_deferral() {
        assert!(
            info("^ = () -> $ => 1 |> @sleep")
                .expect("ok")
                .uses_deferral
        );
    }
}
