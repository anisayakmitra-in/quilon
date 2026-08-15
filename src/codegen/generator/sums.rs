//! Sum types as tagged unions: constructing a variant, and the struct layouts that
//! carry a tag plus its payload.
//!
//! Part of the LLVM code generator; see `super` for the `CodeGenerator` state these
//! methods run against.

use super::*;

impl<'ctx> CodeGenerator<'ctx> {
    pub(super) fn generate_sum_constructor(
        &mut self,
        tag: u8,
        type_name: &str,
        args: &[Expr],
    ) -> Result<BasicValueEnum<'ctx>, String> {
        // Tagged-union value: { i8 tag, slot0, slot1, ... }.
        //
        // The slot types come from one of two sources:
        //  - USER sum types have a registered canonical layout (`sum_layouts`), sized to
        //    the widest variant, so EVERY value of the type shares one struct shape and a
        //    match arm can extract any variant's slots without going out of range:
        //      Rect(3, 4) -> { i8 1, double 3.0, double 4.0 }
        //      Circle(9)  -> { i8 0, double 9.0, double <undef> }   (slot 1 unused)
        //  - `Result` has NO registered layout: it's sized to the actual payload value,
        //    preserving the historical per-value representation across its generic,
        //    possibly-heterogeneous variants:
        //      Ok(42)       -> { i8 0, double 42.0 }
        //      NotOk("err") -> { i8 1, ptr <str> }
        //
        // Num/Bool payloads are normalized to f64. A `$` (Unit) payload is zero-sized; it
        // is stored as a zero of the slot type so the value still matches the slot/return
        // shape (e.g. `Ok($)` -> { i8 0, double 0.0 }) — the bits are never read.
        let i8_type = self.context.i8_type();
        let f64_type = self.context.f64_type();
        let registered_layout = self.sum_layouts.get(type_name).cloned();

        let tag_val = i8_type.const_int(tag as u64, false);

        // Determine each payload slot's value and type. For a registered layout, the slot
        // type is fixed by position; otherwise (Result) it follows the value, with a `$`
        // payload defaulting to the canonical `double` slot.
        let mut payload_vals: Vec<BasicValueEnum> = Vec::with_capacity(args.len());
        for (pos, arg) in args.iter().enumerate() {
            let arg_val = self.generate_expr(arg)?;
            // With a registered layout (user type), the slot type is fixed by position.
            // Without one (Result), the slot follows the value's own type so a Text/Bool
            // payload keeps its real representation — except a `$` (Unit) value, which is
            // zero-sized and defaults to the canonical `double` slot.
            let slot_ty = match registered_layout.as_ref().and_then(|l| l.get(pos).copied()) {
                Some(ty) => ty,
                None if self.expr_is_unit(arg) => f64_type.into(),
                None => self.payload_slot_type(arg_val),
            };
            payload_vals.push(self.coerce_payload(arg_val, slot_ty)?);
        }

        // Build the struct type: tag + (registered layout, or the actual payload types).
        let mut field_types: Vec<BasicTypeEnum> = vec![i8_type.into()];
        match &registered_layout {
            Some(layout) => field_types.extend(layout.iter().copied()),
            None => field_types.extend(payload_vals.iter().map(|v| v.get_type())),
        }
        let sum_struct = self.context.struct_type(&field_types, false);

        let mut agg = sum_struct.get_undef();
        agg = self
            .builder
            .build_insert_value(agg, tag_val, 0, "with_tag")
            .map_err(ctx("Failed to insert tag"))?
            .into_struct_value();
        // Fill the leading slots with this variant's payloads; trailing slots (unused by
        // this variant, in a wider registered layout) stay `undef` — they're only read by
        // an arm matching a different, wider variant, which never runs for this value.
        for (i, payload) in payload_vals.iter().enumerate() {
            agg = self
                .builder
                .build_insert_value(agg, *payload, (i + 1) as u32, "with_payload")
                .map_err(ctx("Failed to insert payload"))?
                .into_struct_value();
        }

        Ok(agg.into())
    }

    /// The slot type for a Result payload sized to its actual value: a non-`i1` integer
    /// widens to f64 (the canonical numeric payload), everything else keeps its own type.
    pub(super) fn payload_slot_type(&self, value: BasicValueEnum<'ctx>) -> BasicTypeEnum<'ctx> {
        match value {
            BasicValueEnum::IntValue(i) if i.get_type().get_bit_width() != 1 => {
                self.context.f64_type().into()
            }
            other => other.get_type(),
        }
    }

    /// Coerce a payload argument value to its target slot type. Integers (incl. the unit
    /// `i8`) widen to f64 for a numeric slot; a `$` (Unit) value targeting a non-`i8` slot
    /// becomes a zero of that slot type (it carries no information). Otherwise the value
    /// is stored as-is (e.g. a Text struct into a Text slot).
    pub(super) fn coerce_payload(
        &self,
        value: BasicValueEnum<'ctx>,
        slot_ty: BasicTypeEnum<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        match value {
            BasicValueEnum::IntValue(i) if slot_ty.is_float_type() => Ok(self
                .builder
                .build_unsigned_int_to_float(i, slot_ty.into_float_type(), "inttofloat")
                .map_err(ctx("Failed to convert payload to float"))?
                .into()),
            // A value already matching the slot type passes through unchanged.
            other if other.get_type() == slot_ty => Ok(other),
            // A `$` (Unit) value — the zero `i8` — carries no information; stored into a
            // differently-typed slot it becomes that slot's zero (e.g. a `$` payload in a
            // `Done($) / Pending(Text)` Text slot). The type checker guarantees concrete
            // payload types agree per position, so ANY other mismatch is an internal bug,
            // surfaced rather than silently zeroed.
            BasicValueEnum::IntValue(i) if i.get_type().get_bit_width() == 8 => Ok(zeroed(slot_ty)),
            other => Err(format!(
                "internal error: sum-type payload of type {:?} does not fit slot {:?}",
                other.get_type(),
                slot_ty
            )),
        }
    }

    /// The `{ i8 tag, elem }` struct that `find`/`at` return — a per-element-typed
    /// `Result` whose single payload slot holds the element (matching the Result-style
    /// per-value layout the pattern-match consumer extracts from field 1).
    pub(super) fn result_struct_type(
        &self,
        elem_llvm: BasicTypeEnum<'ctx>,
    ) -> inkwell::types::StructType<'ctx> {
        self.context
            .struct_type(&[self.context.i8_type().into(), elem_llvm], false)
    }

    /// Build the `{ i8 tag, payload }` value that `find`/`at` return, tagged as Result
    /// variant `variant` (`"Ok"` / `"NotOk"`). The tag number is read from the shared
    /// sum-variant registry (`register_builtin_sum_types`) — the same source the
    /// pattern-match consumer uses — so construction and matching can never drift apart.
    pub(super) fn build_result(
        &mut self,
        elem_llvm: BasicTypeEnum<'ctx>,
        variant: &str,
        payload: BasicValueEnum<'ctx>,
    ) -> BasicValueEnum<'ctx> {
        let tag = self
            .sum_variants
            .get(variant)
            .map(|(t, _)| *t)
            .unwrap_or_else(|| panic!("Result variant `{variant}` is not registered"));
        let struct_ty = self.result_struct_type(elem_llvm);
        let tag_val = self.context.i8_type().const_int(tag as u64, false);
        let mut agg = struct_ty.get_undef();
        agg = self
            .builder
            .build_insert_value(agg, tag_val, 0, "res_tag")
            .expect("insert result tag")
            .into_struct_value();
        agg = self
            .builder
            .build_insert_value(agg, payload, 1, "res_payload")
            .expect("insert result payload")
            .into_struct_value();
        agg.into()
    }

    /// The tagged-union LLVM struct for a sum type: `{ i8 tag, slot0, slot1, ... }`,
    /// where the slots come from the registered canonical payload layout. Falls back to
    /// the Result-style `{ i8, double }` for an unregistered name (e.g. a `-> Result`
    /// annotation reached before any user declaration), keeping the historical shape.
    pub(super) fn sum_struct_type(&self, name: &str) -> inkwell::types::StructType<'ctx> {
        let mut field_types: Vec<BasicTypeEnum> = vec![self.context.i8_type().into()];
        match self.sum_layouts.get(name) {
            Some(layout) => field_types.extend(layout.iter().copied()),
            None => field_types.push(self.context.f64_type().into()),
        }
        self.context.struct_type(&field_types, false)
    }

    /// The tagged-union LLVM struct for a sum-typed *value* of type `Type::Sum`. A USER
    /// sum type has a registered canonical layout, so this defers to [`sum_struct_type`].
    /// The built-in `Result` has NONE (its payload is sized per value across its generic,
    /// heterogeneous variants), so its slot types are recovered from the CONCRETE
    /// (specialized) variant payloads this `Type::Sum` carries: `Result[Ok(Text)]` =>
    /// `{ i8, Text }`, so a function returning `Ok("x")` gets a return type matching the
    /// value the body actually produces.
    ///
    /// This MUST agree with `generate_sum_constructor`'s per-value Result shape: there a
    /// `Generic` slot has no value and a `$` (Unit) payload is stored into the canonical
    /// numeric `double` slot (a Unit carries no bits). So per slot we take the first field
    /// that is neither `Generic` NOR `Unit` (the checker guarantees concrete fields at a
    /// position agree) and lower it via [`value_repr_type`]; a slot that is only
    /// generic/unit/absent falls back to `double`, preserving the historical
    /// `{ i8, double }` shape for a still-generic or unit-only `Result`.
    pub(super) fn sum_value_struct_type(
        &self,
        name: &str,
        variants: &[crate::ast::SumVariant],
    ) -> Result<inkwell::types::StructType<'ctx>, String> {
        if self.sum_layouts.contains_key(name) || variants.is_empty() {
            return Ok(self.sum_struct_type(name));
        }
        let mut field_types: Vec<BasicTypeEnum> = vec![self.context.i8_type().into()];
        let max_fields = variants.iter().map(|v| v.fields.len()).max().unwrap_or(0);
        for i in 0..max_fields {
            let concrete = variants
                .iter()
                .filter_map(|v| v.fields.get(i))
                .find(|f| !matches!(f, Type::Generic { .. } | Type::Unit));
            let slot = match concrete {
                Some(f) => self.value_repr_type(f)?,
                None => self.context.f64_type().into(),
            };
            field_types.push(slot);
        }
        Ok(self.context.struct_type(&field_types, false))
    }
}
