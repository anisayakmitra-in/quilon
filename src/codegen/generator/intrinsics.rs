//! Runtime intrinsics: their LLVM declarations, and the lowering of the I/O and exit
//! builtins onto them.
//!
//! Part of the LLVM code generator; see `super` for the `CodeGenerator` state these
//! methods run against.

use super::*;

impl<'ctx> CodeGenerator<'ctx> {
    /// Declare (once) and return an external runtime intrinsic by its
    /// Quilon-internal name. These resolve to `#[no_mangle]` symbols in
    /// `src/runtime/intrinsics.rs` (or libc, e.g. `memcpy`) — available both to
    /// the in-process JIT and to AOT-linked executables.
    pub(super) fn get_intrinsic(&self, name: &str) -> Result<FunctionValue<'ctx>, String> {
        if let Some(f) = self.module.get_function(name) {
            return Ok(f);
        }
        let ctx = self.context;
        let ptr = ctx.ptr_type(AddressSpace::default());
        let i64t = ctx.i64_type();
        let f64t = ctx.f64_type();
        let void = ctx.void_type();
        let fn_type = match name {
            // i8* __alloc(i64) — GC-managed allocation.
            "__alloc" => ptr.fn_type(&[i64t.into()], false),
            // void __gc_init() — initialize the Boehm GC.
            "__gc_init" => void.fn_type(&[], false),
            // void __exit(i32 code) — terminate the process with `code`. Backs the
            // `__exit(n)` primitive that `core.test`'s `assert` calls to fail. Never
            // returns (the runtime calls libc `exit`).
            "__exit" => void.fn_type(&[ctx.i32_type().into()], false),
            // void __index_fail(double index, i64 size) — report an invalid array index
            // (out of bounds / negative / NaN) to stderr and terminate with status 1.
            // Never returns; codegen emits `unreachable` after the call.
            "__index_fail" => void.fn_type(&[f64t.into(), i64t.into()], false),
            // i8* memcpy(i8*, i8*, i64) — libc.
            "memcpy" => ptr.fn_type(&[ptr.into(), ptr.into(), i64t.into()], false),
            // i64 __text_length(i8*, i64) — grapheme-cluster count.
            "__text_length" => i64t.fn_type(&[ptr.into(), i64t.into()], false),
            // i32 __text_cmp(i8* a, i64 alen, i8* b, i64 blen) — lexicographic byte
            // comparison, returning -1 / 0 / 1. Backs Text ==/!=/</<=/>/>=.
            "__text_cmp" => ctx
                .i32_type()
                .fn_type(&[ptr.into(), i64t.into(), ptr.into(), i64t.into()], false),
            // i64 __write_bytes(i64 fd, i8* ptr, i64 len) — raw write, backs `write`.
            "__write_bytes" => i64t.fn_type(&[i64t.into(), ptr.into(), i64t.into()], false),
            // void __print_num_fd(i64 fd, double) — number + newline to fd.
            "__print_num_fd" => void.fn_type(&[i64t.into(), f64t.into()], false),
            // void __print_bool_fd(i64 fd, i64 b) — "true"/"false" + newline to fd.
            "__print_bool_fd" => void.fn_type(&[i64t.into(), i64t.into()], false),
            // void __print_text_fd(i64 fd, i8*) — C string + newline to fd.
            "__print_text_fd" => void.fn_type(&[i64t.into(), ptr.into()], false),
            // { ptr, i64 } __argv_to_text_array(i64 argc, i8** argv) — build a `[]Text`
            // (array of `{ptr,i64}` Text structs) from the C argc/argv. Returns the
            // `[]Text` value struct (same shape as `ptr_len_struct_type`).
            "__argv_to_text_array" => self
                .ptr_len_struct_type()
                .fn_type(&[i64t.into(), ptr.into()], false),
            // { ptr, i64 } __envp_to_pairs(i8** envp) — build a `[][]Text` (array of
            // 2-element `[]Text` `[key, value]` pairs) from the C envp.
            "__envp_to_pairs" => self.ptr_len_struct_type().fn_type(&[ptr.into()], false),
            // Text methods. A `Text`/`[]Text` result is the `{ ptr, i64 }` struct; a
            // `Text` argument is passed as its (ptr, i64) fields. See `quilon-rt`.
            // { ptr, i64 } trimStart / trimEnd / toUpper / toLower (i8*, i64). `trim` is
            // composed from trimStart+trimEnd in codegen, so it has no own intrinsic.
            "__text_trim_start" | "__text_trim_end" | "__text_to_upper" | "__text_to_lower" => self
                .ptr_len_struct_type()
                .fn_type(&[ptr.into(), i64t.into()], false),
            // i64 __text_contains / __text_index_of (i8* hay, i64, i8* sub, i64).
            "__text_contains" | "__text_index_of" => {
                i64t.fn_type(&[ptr.into(), i64t.into(), ptr.into(), i64t.into()], false)
            }
            // { ptr, i64 } __text_split(i8* hay, i64, i8* sep, i64) -> `[]Text`.
            "__text_split" => self
                .ptr_len_struct_type()
                .fn_type(&[ptr.into(), i64t.into(), ptr.into(), i64t.into()], false),
            // { ptr, i64 } __text_slice(i8*, i64, i64 start, i64 end).
            "__text_slice" => self
                .ptr_len_struct_type()
                .fn_type(&[ptr.into(), i64t.into(), i64t.into(), i64t.into()], false),
            // { ptr, i64 } __text_replace_all(i8* hay,i64, i8* from,i64, i8* to,i64).
            "__text_replace_all" => self.ptr_len_struct_type().fn_type(
                &[
                    ptr.into(),
                    i64t.into(),
                    ptr.into(),
                    i64t.into(),
                    ptr.into(),
                    i64t.into(),
                ],
                false,
            ),
            // { ptr, i64 } __text_replace_n(i8* hay,i64, i8* from,i64, i8* to,i64, i64 count).
            "__text_replace_n" => self.ptr_len_struct_type().fn_type(
                &[
                    ptr.into(),
                    i64t.into(),
                    ptr.into(),
                    i64t.into(),
                    ptr.into(),
                    i64t.into(),
                    i64t.into(),
                ],
                false,
            ),
            other => return Err(format!("Unknown runtime intrinsic: {}", other)),
        };
        Ok(self.module.add_function(name, fn_type, None))
    }

    /// Lower a `print`/`eprint` builtin call: render the single argument's text
    /// and write it, followed by a newline, to stdout (`print`, fd 1) or stderr
    /// (`eprint`, fd 2). Dispatches on the LLVM type of the argument: floats print
    /// as numbers, Text structs / pointers as C strings, integers (incl. bools)
    /// widen to numbers. Yields `Num` 0, so it is usable in expression position.
    pub(super) fn generate_print(
        &mut self,
        name: &str,
        args: &[Expr],
    ) -> Result<BasicValueEnum<'ctx>, String> {
        if args.len() != 1 {
            return Err(format!(
                "{} expects exactly 1 argument, got {}",
                name,
                args.len()
            ));
        }
        let fd = if name == "eprint" { 2 } else { 1 };
        let fd_val = self.context.i64_type().const_int(fd, false);
        let val = self.generate_expr(&args[0])?;
        let (intrinsic, arg): (&str, inkwell::values::BasicMetadataValueEnum) = match val {
            BasicValueEnum::FloatValue(f) => ("__print_num_fd", f.into()),
            // Text is { ptr data, i64 len }; print its NUL-terminated `data`.
            BasicValueEnum::StructValue(s) => {
                let data = self
                    .builder
                    .build_extract_value(s, 0, "text_data")
                    .map_err(ctx("Failed to extract text data"))?
                    .into_pointer_value();
                ("__print_text_fd", data.into())
            }
            // A bare pointer (C string) prints as text.
            BasicValueEnum::PointerValue(p) => ("__print_text_fd", p.into()),
            // A Bool (i1) prints as "true"/"false"; any wider int widens to a number.
            BasicValueEnum::IntValue(i) if i.get_type().get_bit_width() == 1 => {
                let b = self
                    .builder
                    .build_int_z_extend(i, self.context.i64_type(), "bool_ext")
                    .map_err(ctx("Failed to extend bool for print"))?;
                ("__print_bool_fd", b.into())
            }
            BasicValueEnum::IntValue(i) => {
                let f = self
                    .builder
                    .build_unsigned_int_to_float(i, self.context.f64_type(), "print_num")
                    .map_err(ctx("Failed to convert int for print"))?;
                ("__print_num_fd", f.into())
            }
            other => {
                return Err(format!(
                    "print does not support a value of type {:?}",
                    other.get_type()
                ));
            }
        };
        let print_fn = self.get_intrinsic(intrinsic)?;
        self.builder
            .build_call(print_fn, &[fd_val.into(), arg], "")
            .map_err(ctx("Failed to build print call"))?;
        // `print`/`eprint` yield Unit (`$`); their result is meaningless.
        Ok(self.unit_value().into())
    }

    /// Lower the `__exit(code)` primitive: convert the `Num` `code` to an `i32` and
    /// call the `__exit` runtime intrinsic, which terminates the process. This is the
    /// single native primitive `core.test` builds on (its `assert` calls `__exit(101)`
    /// on failure). The intrinsic never returns, but the call is left as ordinary
    /// (non-`unreachable`) flow so it composes wherever an expression is expected —
    /// e.g. a `< >` block statement or a ternary arm inside `assert` — without
    /// clashing with the surrounding construct's own terminator. The code after it is
    /// dead at runtime (the process has exited). Yields `$` (Unit).
    pub(super) fn generate_exit(&mut self, args: &[Expr]) -> Result<BasicValueEnum<'ctx>, String> {
        if args.len() != 1 {
            return Err(format!(
                "__exit expects exactly 1 argument, got {}",
                args.len()
            ));
        }
        let code = self.generate_expr(&args[0])?;
        let BasicValueEnum::FloatValue(code_f) = code else {
            return Err("__exit expects a Num exit code".to_string());
        };
        let code_i32 = self
            .builder
            .build_float_to_signed_int(code_f, self.context.i32_type(), "exit_code")
            .map_err(ctx("Failed to convert __exit code"))?;
        let exit_fn = self.get_intrinsic("__exit")?;
        self.builder
            .build_call(exit_fn, &[code_i32.into()], "")
            .map_err(ctx("Failed to build __exit call"))?;
        // `__exit` never returns; yield Unit so the call composes in expression position.
        Ok(self.unit_value().into())
    }

    /// Lower the `write(content, fd)` builtin: write the raw bytes of a `Text`
    /// `content` to file descriptor `fd` (a `Num`), with no trailing newline.
    /// Yields `Num` (bytes written).
    pub(super) fn generate_write(&mut self, args: &[Expr]) -> Result<BasicValueEnum<'ctx>, String> {
        if args.len() != 2 {
            return Err(format!(
                "write expects exactly 2 arguments (content, fd), got {}",
                args.len()
            ));
        }
        let content = self.generate_expr(&args[0])?;
        let fd_num = self.generate_expr(&args[1])?;
        // content must be a Text { ptr data, i64 byte_len }.
        let s = match content {
            BasicValueEnum::StructValue(s) => s,
            other => {
                return Err(format!(
                    "write expects a Text content, got {:?}",
                    other.get_type()
                ));
            }
        };
        let data = self
            .builder
            .build_extract_value(s, 0, "write_data")
            .map_err(ctx("Failed to extract text data"))?
            .into_pointer_value();
        let len = self
            .builder
            .build_extract_value(s, 1, "write_len")
            .map_err(ctx("Failed to extract text len"))?
            .into_int_value();
        let fd_float = match fd_num {
            BasicValueEnum::FloatValue(f) => f,
            other => {
                return Err(format!(
                    "write expects a Num fd, got {:?}",
                    other.get_type()
                ));
            }
        };
        let fd_i64 = self
            .builder
            .build_float_to_signed_int(fd_float, self.context.i64_type(), "write_fd")
            .map_err(ctx("Failed to convert fd"))?;
        let write_fn = self.get_intrinsic("__write_bytes")?;
        use inkwell::values::AnyValue;
        let written = self
            .builder
            .build_call(
                write_fn,
                &[fd_i64.into(), data.into(), len.into()],
                "write_n",
            )
            .map_err(ctx("Failed to call __write_bytes"))?
            .as_any_value_enum()
            .into_int_value();
        Ok(self
            .builder
            .build_signed_int_to_float(written, self.context.f64_type(), "write_ret")
            .map_err(ctx("Failed to convert write result"))?
            .into())
    }
}
