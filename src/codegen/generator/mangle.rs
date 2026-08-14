//! Symbol mangling for overload members.
//!
//! Each overload member is emitted under a distinct symbol built from its parameter
//! types, so a call mangles to the same name the definition did.

use super::*;

/// Names that the compiler provides built-in overloads for (`print`/`eprint`, lowered
/// to runtime intrinsics). A user definition of one ADDS an overload member (and is
/// mangled), rather than shadowing the built-in single-arg Num/Text/Bool forms.
pub(super) fn is_builtin_overload_name(name: &str) -> bool {
    matches!(name, "print" | "eprint")
}

/// A short, mangling-safe tag for a Quilon type used in overload name mangling. Must be
/// deterministic and identical at definition and call sites (built from the declared
/// parameter type and from the inferred argument type respectively).
pub(super) fn type_mangle(ty: &Type) -> String {
    match ty {
        Type::Num => "N".to_string(),
        Type::Text => "T".to_string(),
        Type::Bool => "B".to_string(),
        Type::Unit => "U".to_string(),
        Type::Array(elem) => format!("A{}", type_mangle(elem)),
        Type::Named { name, .. } | Type::Sum { name, .. } => format!("named${}", name),
        // A not-yet-concrete sum payload (`Generic`) resolves as `Num` for overload
        // dispatch (see the type checker's `types_match`), so it mangles to the Num tag
        // — keeping codegen's chosen symbol in agreement with the checker.
        Type::Generic { .. } => "N".to_string(),
        // Any other shape (e.g. a function type) — a stable, mangling-safe fallback.
        other => format!("X{:?}", other)
            .chars()
            .filter(|c| c.is_alphanumeric() || *c == '$')
            .collect(),
    }
}

/// Render an entry point's declared parameter types as a readable signature fragment
/// (comma-joined `Num`/`Text`/`[]Text`-style labels) for the unsupported-signature
/// diagnostic. `()` renders as an empty string. Uses the shared `ast::type_label` so
/// codegen and the type checker render types identically.
pub(super) fn fmt_param_types(params: &[Type]) -> String {
    params
        .iter()
        .map(crate::ast::type_label)
        .collect::<Vec<_>>()
        .join(", ")
}

/// The distinct LLVM symbol for one overload member: its name plus a per-parameter
/// type tag. Operator symbols (which aren't valid LLVM identifiers) are spelled out so
/// e.g. `+` on `(Point, Point)` becomes `op.add$named$Point$named$Point`.
pub(super) fn mangle_overload(name: &str, params: &[Type]) -> String {
    let base = operator_word(name)
        .map(|w| format!("op.{}", w))
        .unwrap_or_else(|| name.to_string());
    let mut s = base;
    for p in params {
        s.push('$');
        s.push_str(&type_mangle(p));
    }
    s
}

/// A pronounceable word for an operator symbol, for use in a mangled LLVM name (which
/// can't contain the raw symbol). Returns `None` for non-operator (ordinary) names.
pub(super) fn operator_word(name: &str) -> Option<&'static str> {
    Some(match name {
        "+" => "add",
        "-" => "sub",
        "*" => "mul",
        "/" => "div",
        "%" => "mod",
        "==" => "eq",
        "!=" => "ne",
        "<" => "lt",
        "<=" => "le",
        ">" => "gt",
        ">=" => "ge",
        "&&" => "and",
        "||" => "or",
        _ => return None,
    })
}
