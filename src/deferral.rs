//! Deferred-taint pass — the strictness analysis behind Quilon's colorless
//! implicit-futures model.
//!
//! Calling an `@` primitive (currently `@sleep`) launches its IO and yields a *deferred*
//! value immediately; deferral then flows through pure code untouched and is *forced* only
//! where a built-in primitive must read the value's concrete representation. This pass
//! colors, over the type-checked AST, which expressions evaluate to a deferred value — the
//! set codegen consults to (a) emit the promise-capable representation for exactly those
//! values and (b) emit a `force` at each force-set site that consumes one. It reads no
//! types and adds none: **deferral keys off the operation, never the type**, so the type
//! checker stays byte-identical. Pure programs taint nothing, so codegen for them is
//! unchanged — zero overhead.
//!
//! Force keys off the *operation*: the force-set (the strict built-in primitives) forces a
//! deferred operand; everything else threads it lazily. Deferral is introduced by a `@`
//! call and propagates through `=`/`:=` bindings (and reads of those bindings).
//!
//! Scope of the 0.9 slice: deferred values are `Num` scalars (boxed as a promise handle),
//! and the *implemented* force-set is arithmetic/comparison/logical operators, unary
//! operators, an `@`-primitive's own value arguments, `print`/`eprint`'s data argument, and
//! a function's (or the `^` entry's) return value. A deferred value reaching any *other*
//! position (a match scrutinee, a ternary condition, a field/index, a user-function
//! argument, a composite element, …) is rejected here with a clear "not yet supported"
//! diagnostic rather than miscompiled — those positions land in follow-ups as the force-set
//! and the pointer-tagged composite representation are filled in.

use crate::ast::{Expr, Item, Program, Statement, VarDecl};
use crate::lexer::Span;
use std::collections::{HashMap, HashSet};

/// What the pass hands to codegen.
#[derive(Debug, Default, Clone)]
pub struct DeferInfo {
    /// Spans of expressions whose runtime value is a *deferred* handle (a promise), not a
    /// ready scalar. Codegen emits the pointer representation for these and forces them at
    /// force-set sites. Keyed by `Span` exactly like the type oracle.
    pub deferred: HashSet<Span>,
    /// Whether any `@` primitive launch is reachable. Gates the runtime wrapping (running
    /// the entry on a scheduler fiber, `< >` scope join): a program that launches nothing
    /// is byte-identical to before this pass existed.
    pub uses_deferral: bool,
}

impl DeferInfo {
    /// Whether `expr` evaluates to a deferred (promise-represented) value.
    pub fn is_deferred(&self, expr: &Expr) -> bool {
        self.deferred.contains(expr.span())
    }
}

/// A deferred value used in a position the 0.9 slice does not yet force. Carries the span
/// so the front end can render a source-located diagnostic.
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

/// Analyze `program`, returning the deferred-value coloring or the first unsupported use.
pub fn analyze(program: &Program) -> Result<DeferInfo, DeferError> {
    let mut pass = Pass {
        info: DeferInfo::default(),
    };
    for item in &program.items {
        match item {
            Item::FunctionDecl(f) => {
                // A function's (or `^`'s) result is forced at its return, so a deferred
                // body value is allowed here.
                let mut env = Env::new();
                for p in &f.params {
                    env.set(&p.name, false);
                }
                pass.visit_forcing(&f.body, &mut env)?;
            }
            Item::VarDecl(v) => {
                // A top-level binding's value is not itself a force site, but a top-level
                // `@` launch has no enclosing block to join it; keep it simple and force.
                let mut env = Env::new();
                pass.visit_forcing(&v.value, &mut env)?;
            }
            Item::TypeDecl(_) => {}
        }
    }
    Ok(pass.info)
}

/// Lexical environment: variable name -> whether it currently holds a deferred value.
struct Env {
    scopes: Vec<HashMap<String, bool>>,
}

impl Env {
    fn new() -> Self {
        Env {
            scopes: vec![HashMap::new()],
        }
    }
    fn push(&mut self) {
        self.scopes.push(HashMap::new());
    }
    fn pop(&mut self) {
        self.scopes.pop();
    }
    fn set(&mut self, name: &str, deferred: bool) {
        self.scopes
            .last_mut()
            .expect("env always has a scope")
            .insert(name.to_string(), deferred);
    }
    fn get(&self, name: &str) -> bool {
        self.scopes
            .iter()
            .rev()
            .find_map(|s| s.get(name))
            .copied()
            .unwrap_or(false)
    }
}

struct Pass {
    info: DeferInfo,
}

impl Pass {
    /// Visit `expr` and return whether it evaluates to a deferred value. Records the span
    /// of every deferred-valued expression. Callers decide, per position, whether a
    /// deferred result is acceptable ([`visit_forcing`]) or unsupported ([`visit_ready`]).
    fn visit(&mut self, expr: &Expr, env: &mut Env) -> Result<bool, DeferError> {
        let deferred = match expr {
            Expr::Number { .. }
            | Expr::String { .. }
            | Expr::Bool { .. }
            | Expr::Unit { .. } => false,

            Expr::Interpolation { parts, .. } => {
                // Each hole is rendered (a force site).
                for part in parts {
                    if let crate::ast::InterpPart::Hole(e) = part {
                        self.visit_forcing(e, env)?;
                    }
                }
                false
            }

            Expr::Ident { name, .. } => env.get(name),

            Expr::Call { func, args, .. } => self.visit_call(func, args, env)?,

            // Arithmetic / comparison / logical operators all read their operands' bits.
            Expr::BinOp { left, right, .. } => {
                self.visit_forcing(left, env)?;
                self.visit_forcing(right, env)?;
                false
            }
            Expr::UnaryOp { expr, .. } => {
                self.visit_forcing(expr, env)?;
                false
            }

            Expr::Pipeline { left, right, span } => {
                // Desugar exactly as codegen does, then analyze the resulting call, so a
                // pipeline into an `@` primitive (`10 |> @sleep`) is a launch.
                let call = Expr::desugar_pipeline(left, right, span);
                self.visit(&call, env)?
            }

            Expr::Block { stmts, .. } => self.visit_block(stmts, env)?,

            Expr::If {
                cond, then, else_, ..
            } => {
                self.visit_ready(cond, env, "a condition")?;
                self.visit_ready(then, env, "a ternary/if branch")?;
                self.visit_ready(else_, env, "a ternary/if branch")?;
                false
            }

            Expr::Match { expr, arms, .. } => {
                self.visit_ready(expr, env, "a match scrutinee")?;
                for arm in arms {
                    self.visit_ready(&arm.body, env, "a match arm")?;
                }
                false
            }

            Expr::FieldAccess { expr, .. } => {
                self.visit_ready(expr, env, "a field access")?;
                false
            }
            Expr::FieldAssign { target, value, .. } => {
                self.visit_ready(target, env, "a field assignment target")?;
                self.visit_ready(value, env, "a field assignment value")?;
                false
            }
            Expr::Index { expr, index, .. } => {
                self.visit_ready(expr, env, "an index receiver")?;
                self.visit_ready(index, env, "an array index")?;
                false
            }
            Expr::Array { elements, .. } => {
                for e in elements {
                    self.visit_ready(e, env, "an array element")?;
                }
                false
            }
            Expr::Record { fields, .. } => {
                for (_, e) in fields {
                    self.visit_ready(e, env, "a record field")?;
                }
                false
            }
            Expr::Constructor { fields, .. } => {
                for (_, e) in fields {
                    self.visit_ready(e, env, "a constructor field")?;
                }
                false
            }
            Expr::Range { start, end, .. } => {
                self.visit_ready(start, env, "a range bound")?;
                self.visit_ready(end, env, "a range bound")?;
                false
            }
            Expr::Spread { expr, .. } => {
                self.visit_ready(expr, env, "a spread source")?;
                false
            }
            Expr::Lambda { body, params, .. } => {
                // Analyze the body under a fresh scope; a lambda param is never deferred.
                env.push();
                for p in params {
                    env.set(&p.name, false);
                }
                self.visit_forcing(body, env)?;
                env.pop();
                false
            }
        };

        if deferred {
            self.info.deferred.insert(expr.span().clone());
        }
        Ok(deferred)
    }

    fn visit_call(
        &mut self,
        func: &Expr,
        args: &[Expr],
        env: &mut Env,
    ) -> Result<bool, DeferError> {
        let name = match func {
            Expr::Ident { name, .. } => Some(name.as_str()),
            _ => None,
        };

        // A deferring `@` primitive: its value arguments are a force site, and the call
        // itself yields a deferred value.
        if name.is_some_and(|n| n.starts_with('@')) {
            for arg in args {
                self.visit_forcing(arg, env)?;
            }
            self.info.uses_deferral = true;
            return Ok(true);
        }

        // `print`/`eprint` render (force) their single data argument.
        if matches!(name, Some("print" | "eprint")) {
            for arg in args {
                self.visit_forcing(arg, env)?;
            }
            return Ok(false);
        }

        // Any other call (user function, operator overload, method, `write`, sum
        // constructor): its arguments are not yet a force site, so a deferred argument is
        // unsupported in this slice. The result is treated as ready.
        for arg in args {
            self.visit_ready(arg, env, "a function argument")?;
        }
        Ok(false)
    }

    fn visit_block(&mut self, stmts: &[Statement], env: &mut Env) -> Result<bool, DeferError> {
        env.push();
        let mut result = false;
        for (i, stmt) in stmts.iter().enumerate() {
            let is_last = i + 1 == stmts.len();
            match stmt {
                Statement::Item(Item::VarDecl(VarDecl { name, value, .. })) => {
                    // A binding is the propagation site: the bound name inherits its value's
                    // deferral. The value itself is analyzed lazily (not forced).
                    let d = self.visit(value, env)?;
                    env.set(name, d);
                    result = false;
                }
                Statement::Item(item) => {
                    self.visit_item(item, env)?;
                    result = false;
                }
                Statement::Expr(e) => {
                    // A non-final expression statement is evaluated for effect; a deferred
                    // launch there is fine (the enclosing scope joins it). The final
                    // expression is the block's value; its deferral flows out.
                    let d = self.visit(e, env)?;
                    result = if is_last { d } else { false };
                }
            }
        }
        env.pop();
        Ok(result)
    }

    fn visit_item(&mut self, item: &Item, env: &mut Env) -> Result<(), DeferError> {
        match item {
            Item::FunctionDecl(f) => {
                env.push();
                for p in &f.params {
                    env.set(&p.name, false);
                }
                self.visit_forcing(&f.body, env)?;
                env.pop();
            }
            Item::VarDecl(v) => {
                self.visit_forcing(&v.value, env)?;
            }
            Item::TypeDecl(_) => {}
        }
        Ok(())
    }

    /// Visit an expression in a position that forces it if deferred (a force-set site or a
    /// function/entry return). A deferred result is acceptable — codegen emits the force.
    fn visit_forcing(&mut self, expr: &Expr, env: &mut Env) -> Result<(), DeferError> {
        self.visit(expr, env)?;
        Ok(())
    }

    /// Visit an expression in a position that needs a ready value but that the 0.9 slice
    /// does not yet force. A deferred result is an error (rejected, never miscompiled).
    fn visit_ready(&mut self, expr: &Expr, env: &mut Env, position: &str) -> Result<(), DeferError> {
        if self.visit(expr, env)? {
            return Err(DeferError {
                span: expr.span().clone(),
                message: format!(
                    "a deferred value (from an `@` primitive) flows into {position}, which \
                     is not supported yet — force it first by using it in arithmetic, a \
                     comparison, or `print` (the 0.9 force-set), or bind it and force that. \
                     Wider force-set support is coming in a follow-up."
                ),
            });
        }
        Ok(())
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
    fn pure_program_taints_nothing() {
        let i = info("^ = () -> Num => 1 + 2 * 3").expect("ok");
        assert!(!i.uses_deferral);
        assert!(i.deferred.is_empty());
    }

    #[test]
    fn sleep_launch_and_binding_reads_are_deferred() {
        let src = "^ = () -> Num => <\n  a = @sleep(10)\n  b = @sleep(20)\n  a + b\n>";
        let i = info(src).expect("ok");
        assert!(i.uses_deferral);
        // Two launches + two deferred reads of `a` and `b` = 4 deferred spans.
        assert_eq!(i.deferred.len(), 4);
    }

    #[test]
    fn direct_launch_operands_force_at_arithmetic() {
        let i = info("^ = () -> Num => @sleep(10) + @sleep(20)").expect("ok");
        assert!(i.uses_deferral);
        // The two `@sleep(...)` calls are deferred; the `+` result is ready.
        assert_eq!(i.deferred.len(), 2);
    }

    #[test]
    fn non_deferred_binding_is_not_tainted() {
        let src = "^ = () -> Num => <\n  a = @sleep(10)\n  c = 5\n  a + c\n>";
        let i = info(src).expect("ok");
        // `a` launch + `a` read are deferred; `c` and its read are not.
        assert_eq!(i.deferred.len(), 2);
    }

    #[test]
    fn deferred_into_unsupported_position_is_rejected() {
        // A deferred value as a match scrutinee is not in the 0.9 force-set.
        let src = "^ = () -> Num => <\n  a = @sleep(10)\n  a ? | _ => 0\n>";
        assert!(info(src).is_err());
    }

    #[test]
    fn bound_but_unused_launch_still_tracked() {
        let src = "^ = () -> Num => <\n  x = @sleep(50)\n  0\n>";
        let i = info(src).expect("ok");
        assert!(i.uses_deferral);
        // Only the launch is deferred (never read); scope-join forces it at runtime.
        assert_eq!(i.deferred.len(), 1);
    }

    /// The load-bearing guardrail: deferral is **type-invisible**. Every expression this
    /// pass colors deferred is recorded by the (unmodified) type checker as an ordinary
    /// `Num` — there is no `Task`/`Future`/`Deferred` type. If deferral ever leaked into
    /// the type system, a deferred expression's checker type would stop being a plain
    /// `Num` and this fails.
    #[test]
    fn deferred_values_carry_ordinary_types_in_the_checker() {
        use crate::modules;
        use crate::typechecker::TypeChecker;
        use std::path::Path;

        let src = "<< core.time\n^ = () -> Num => <\n  a = @sleep(10)\n  b = @sleep(20)\n  a + b\n>";
        let tokens = Lexer::tokenize(src).expect("lex");
        let program = parser::parse(&tokens).expect("parse");
        let program = modules::link(program, Path::new(".")).expect("link core.time");

        let types = TypeChecker::new()
            .check_program(&program)
            .expect("type checking");
        let info = analyze(&program).expect("analyze");

        assert!(info.uses_deferral);
        assert!(!info.deferred.is_empty());
        for span in &info.deferred {
            assert_eq!(
                types.get(span),
                Some(&crate::ast::Type::Num),
                "a deferred expression must type as an ordinary Num (deferral is invisible \
                 to the checker)"
            );
        }
    }
}
