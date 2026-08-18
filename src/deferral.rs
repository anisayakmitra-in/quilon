//! Deferral analysis — the compiler's view of Quilon's `@` leaf-IO-primitive tier.
//!
//! Today its one job is small and exact: decide whether a program reaches any `@` primitive
//! (`uses_deferral`), so codegen runs the entry on a scheduler fiber only then — a pure
//! program is compiled exactly as before, with zero overhead. It reads no types and adds
//! none, so the type checker is unaffected: an `@` primitive keeps its ordinary result type
//! (`@sleep` is plain `$`). A top-level binding that tries to call an `@` primitive is
//! already rejected upstream (nothing runs before `^` to compute it), so there is nothing to
//! validate here.
//!
//! This is the growth point for the fuller strictness analysis (the deferred-value taint and
//! force-frontier) that arrives with a value-returning primitive (`@read`).

use crate::ast::{Expr, InterpPart, Item, Program, Statement};

/// What the analysis hands to codegen.
#[derive(Debug, Default, Clone)]
pub struct DeferInfo {
    /// Whether any `@` leaf IO primitive is reachable. Gates running the entry on a
    /// scheduler fiber: a program that uses no `@` primitive is byte-identical to before.
    pub uses_deferral: bool,
}

/// Analyze `program`: report whether it reaches any `@` leaf IO primitive.
pub fn analyze(program: &Program) -> DeferInfo {
    let uses_deferral = program.items.iter().any(|item| match item {
        Item::FunctionDecl(f) => references_at_primitive(&f.body),
        Item::VarDecl(v) => references_at_primitive(&v.value),
        Item::TypeDecl(_) => false,
    });
    DeferInfo { uses_deferral }
}

/// Whether any `@`-primitive reference appears anywhere in `expr`. An `@` name can only ever
/// name a leaf IO primitive (the parser reserves the `@`), so any `@`-prefixed identifier —
/// called directly, piped into, or otherwise — counts.
fn references_at_primitive(expr: &Expr) -> bool {
    match expr {
        Expr::Ident { name, .. } => name.starts_with('@'),
        Expr::Number { .. } | Expr::String { .. } | Expr::Bool { .. } | Expr::Unit { .. } => false,

        Expr::Interpolation { parts, .. } => parts.iter().any(|p| match p {
            InterpPart::Hole(e) => references_at_primitive(e),
            InterpPart::Lit(_) => false,
        }),
        Expr::Call { func, args, .. } => {
            references_at_primitive(func) || args.iter().any(references_at_primitive)
        }
        Expr::BinOp { left, right, .. } | Expr::Pipeline { left, right, .. } => {
            references_at_primitive(left) || references_at_primitive(right)
        }
        Expr::Range { start, end, .. } => {
            references_at_primitive(start) || references_at_primitive(end)
        }
        Expr::UnaryOp { expr, .. }
        | Expr::FieldAccess { expr, .. }
        | Expr::Spread { expr, .. }
        | Expr::Lambda { body: expr, .. } => references_at_primitive(expr),
        Expr::FieldAssign { target, value, .. } => {
            references_at_primitive(target) || references_at_primitive(value)
        }
        Expr::Index { expr, index, .. } => {
            references_at_primitive(expr) || references_at_primitive(index)
        }
        Expr::If {
            cond, then, else_, ..
        } => {
            references_at_primitive(cond)
                || references_at_primitive(then)
                || references_at_primitive(else_)
        }
        Expr::Match { expr, arms, .. } => {
            references_at_primitive(expr) || arms.iter().any(|a| references_at_primitive(&a.body))
        }
        Expr::Array { elements, .. } => elements.iter().any(references_at_primitive),
        Expr::Record { fields, .. } | Expr::Constructor { fields, .. } => {
            fields.iter().any(|(_, e)| references_at_primitive(e))
        }
        Expr::Block { stmts, .. } => stmts.iter().any(|s| match s {
            Statement::Expr(e) => references_at_primitive(e),
            Statement::Item(Item::VarDecl(v)) => references_at_primitive(&v.value),
            Statement::Item(Item::FunctionDecl(f)) => references_at_primitive(&f.body),
            Statement::Item(Item::TypeDecl(_)) => false,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::Lexer;
    use crate::parser;

    fn uses_deferral(src: &str) -> bool {
        let tokens = Lexer::tokenize(src).expect("lex");
        let program = parser::parse(&tokens).expect("parse");
        analyze(&program).uses_deferral
    }

    #[test]
    fn pure_program_uses_no_deferral() {
        assert!(!uses_deferral("^ = () -> Num => 1 + 2 * 3"));
    }

    #[test]
    fn a_sleep_call_marks_deferral() {
        assert!(uses_deferral("^ = () -> $ => <\n  @sleep(1)\n  $\n>"));
    }

    #[test]
    fn sleep_reached_through_a_helper_marks_deferral() {
        assert!(uses_deferral(
            "nap = () -> $ => @sleep(1)\n^ = () -> $ => nap()"
        ));
    }

    #[test]
    fn sleep_piped_in_still_marks_deferral() {
        assert!(uses_deferral("^ = () -> $ => 1 |> @sleep"));
    }
}
