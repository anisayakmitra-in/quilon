//! Call lowering: resolving what a call names (function, overload member, method, or
//! intrinsic) and emitting the call itself.
//!
//! Part of the LLVM code generator; see `super` for the `CodeGenerator` state these
//! methods run against.

use super::*;

impl<'ctx> CodeGenerator<'ctx> {
    /// Emit a call to `function` with argument values that are already generated, and
    /// yield its result. The one place a direct call is built, shared by `generate_call`
    /// (which resolves the callee from a name) and the tail-call lowering.
    pub(super) fn emit_call(
        &mut self,
        function: inkwell::values::FunctionValue<'ctx>,
        arg_values: &[BasicValueEnum<'ctx>],
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let arg_metadata: Vec<inkwell::values::BasicMetadataValueEnum> =
            arg_values.iter().map(|v| (*v).into()).collect();
        let call_site = self
            .builder
            .build_call(function, &arg_metadata, "calltmp")
            .map_err(ctx("Failed to build call"))?;
        Self::call_result_to_basic(call_site)
    }

    pub(super) fn generate_call(
        &mut self,
        func: &Expr,
        args: &[Expr],
    ) -> Result<BasicValueEnum<'ctx>, String> {
        // Get function name - only support direct calls for now
        let func_name = if let Expr::Ident { name, .. } = func {
            name
        } else {
            return Err("Only direct function calls supported".to_string());
        };

        // Core IO builtins, lowered to runtime intrinsics (see runtime::intrinsics).
        // `print`/`eprint` are the built-in single-arg Num/Text/Bool overloads; a
        // *user* overload of the same name (a different signature) is dispatched as a
        // mangled function below, so only use the intrinsic when no user overload
        // matches the argument types.
        match func_name.as_str() {
            "print" | "eprint" => {
                // Any single argument renders through its `` ` `` operator (built-in
                // default or user override); only an EXACT user overload of `print`/`eprint`
                // (a different signature) is dispatched as a mangled call below instead. A
                // function-typed argument is not a renderable value — the type checker
                // already rejects `print(f)` (see `is_generic_print_call`), so it never
                // reaches here and this gate needs no separate `Function` exclusion.
                let arg_types: Vec<Type> = args.iter().map(|a| self.infer_type(a)).collect();
                let has_user_match = self
                    .resolve_overload_symbol(func_name, &arg_types)
                    .is_some();
                if arg_types.len() == 1 && !has_user_match {
                    return self.generate_print(func_name, args);
                }
            }
            "write" => return self.generate_write(args),
            // `__exit(code)` — the single native primitive `core.test` builds on,
            // lowered to the `__exit` runtime intrinsic (terminates the process).
            "__exit" => return self.generate_exit(args),
            _ => {}
        }

        // Built-in array methods (`map`/`filter`/`reduce`/`each`/`find`/`at`) — RESERVED
        // on arrays. The method applies only when the receiver (`args[0]`) is an array;
        // the oracle confirms its element type, so this never diverts a same-named user
        // overload on a non-array receiver. Method names are lowercase and so can never
        // collide with a (Capitalized) sum-constructor name — the relative order of this
        // check and the sum-constructor block below is therefore immaterial.
        if crate::ast::is_array_method(func_name)
            && !args.is_empty()
            && matches!(self.oracle.expr_type(&args[0]), Some(Type::Array(_)))
        {
            return self.generate_array_method(func_name, args);
        }

        // Built-in Text methods — RESERVED on `Text`, mirroring the array-method block:
        // dispatch only when the receiver (`args[0]`) is a `Text` (per the oracle), so a
        // same-named user overload on another type is never diverted. Lowercase/camelCase
        // names never collide with (Capitalized) sum constructors.
        if crate::ast::is_text_method(func_name)
            && !args.is_empty()
            && matches!(self.oracle.expr_type(&args[0]), Some(Type::Text))
        {
            return self.generate_text_method(func_name, args);
        }

        // Sum-type constructor with a payload (e.g. `Ok(x)`, `Circle(r)`, `Rect(w, h)`):
        // resolved from the variant registry built from the predefined Result and all
        // user `TypeDef::Sum` declarations.
        if let Some((tag, type_name)) = self.sum_variants.get(func_name.as_str()).cloned() {
            return self.generate_sum_constructor(tag, &type_name, args);
        }

        // A local variable bound to a closure value: call it indirectly, passing the
        // captured environment as the trailing argument. Recognized by the variable's
        // recorded closure signature (see `closure_sigs`). Checked before overload
        // dispatch — a local closure binding shadows any same-named top-level function.
        if let Some((param_tys, ret_ty)) = self.closure_sigs.get(func_name.as_str()).cloned()
            && self.variables.contains_key(func_name.as_str())
        {
            return self.generate_closure_call(func_name, &param_tys, ret_ty, args);
        }

        // Overloaded function call: dispatch to the per-signature mangled symbol chosen
        // by exact argument types (the type checker has already verified a unique match).
        let overload_symbol = if self.overloads.contains_key(func_name.as_str()) {
            let arg_types: Vec<Type> = args.iter().map(|a| self.infer_type(a)).collect();
            self.resolve_overload_symbol(func_name, &arg_types)
        } else {
            None
        };

        // Get the function from the module. If there is no plain top-level function with this
        // name, it may be a method call: the parser desugars `recv.method(a, b)` to
        // `method(recv, a, b)`, so resolve `recv`'s named type and dispatch to `Type_method`.
        let function = if let Some(sym) = &overload_symbol {
            self.module
                .get_function(sym)
                .ok_or_else(|| format!("Overload not found: {}", sym))?
        } else {
            match self.module.get_function(func_name) {
                Some(f) => f,
                None => {
                    let mangled = args
                        .first()
                        .and_then(|recv| self.receiver_type_name(recv))
                        .map(|type_name| method_symbol(&type_name, func_name));
                    match mangled.and_then(|m| self.module.get_function(&m)) {
                        Some(f) => f,
                        None => return Err(format!("Function not found: {}", func_name)),
                    }
                }
            }
        };

        // Generate argument values
        let arg_values: Vec<BasicValueEnum> = args
            .iter()
            .map(|arg| self.generate_expr(arg))
            .collect::<Result<Vec<_>, _>>()?;

        self.emit_call(function, &arg_values)
    }

    /// Convert a call site's result to a `BasicValueEnum`, erroring if the callee returns
    /// a non-basic (e.g. void) value. Shared by the direct (`generate_call`) and indirect
    /// closure (`generate_closure_call`) call paths so both handle return kinds identically.
    pub(super) fn call_result_to_basic(
        call: inkwell::values::CallSiteValue<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        use inkwell::values::AnyValue;
        match call.as_any_value_enum() {
            inkwell::values::AnyValueEnum::IntValue(v) => Ok(v.into()),
            inkwell::values::AnyValueEnum::FloatValue(v) => Ok(v.into()),
            inkwell::values::AnyValueEnum::PointerValue(v) => Ok(v.into()),
            inkwell::values::AnyValueEnum::ArrayValue(v) => Ok(v.into()),
            inkwell::values::AnyValueEnum::StructValue(v) => Ok(v.into()),
            inkwell::values::AnyValueEnum::VectorValue(v) => Ok(v.into()),
            _ => Err("call did not return a basic value".to_string()),
        }
    }

    /// Resolve the named record type of a method-call receiver, if known. Handles both a
    /// variable holding a constructed instance and a constructor expression used directly.
    pub(super) fn receiver_type_name(&self, expr: &Expr) -> Option<String> {
        match expr {
            Expr::Ident { name, .. } => self.var_named_types.get(name).cloned(),
            Expr::Constructor { type_name, .. } => Some(type_name.clone()),
            _ => None,
        }
    }

    /// Build a direct call to an already-emitted function by symbol, given the
    /// already-generated argument values. Used to lower a resolved operator/function
    /// overload to its mangled target.
    pub(super) fn build_direct_call(
        &mut self,
        symbol: &str,
        arg_values: &[BasicValueEnum<'ctx>],
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let function = self
            .module
            .get_function(symbol)
            .ok_or_else(|| format!("Overload not found: {}", symbol))?;
        let arg_metadata: Vec<inkwell::values::BasicMetadataValueEnum> =
            arg_values.iter().map(|v| (*v).into()).collect();
        use inkwell::values::AnyValue;
        let call_site = self
            .builder
            .build_call(function, &arg_metadata, "calltmp")
            .map_err(ctx("Failed to build call"))?;
        match call_site.as_any_value_enum() {
            inkwell::values::AnyValueEnum::IntValue(v) => Ok(v.into()),
            inkwell::values::AnyValueEnum::FloatValue(v) => Ok(v.into()),
            inkwell::values::AnyValueEnum::PointerValue(v) => Ok(v.into()),
            inkwell::values::AnyValueEnum::ArrayValue(v) => Ok(v.into()),
            inkwell::values::AnyValueEnum::StructValue(v) => Ok(v.into()),
            inkwell::values::AnyValueEnum::VectorValue(v) => Ok(v.into()),
            _ => Err("Overloaded function does not return a basic value".to_string()),
        }
    }
}
