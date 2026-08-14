# Quilon value formatters for LLDB / CodeLLDB.
#
# Imported into the debug session by the Quilon VS Code extension via
# `command script import` (see the launch configuration the extension resolves).
#
# STATUS
# ------
# Line-number debugging (breakpoints, stepping, backtraces that reference `.ql`
# lines) works today off the DWARF line table that `quilon build --debug` emits.
#
# Rich *value* rendering — Text shown as a string, arrays as lists, records as
# field maps, sum types as their variant — needs the compiler to emit DISTINCT
# DWARF types for those Quilon shapes. That debug-types work is NOT merged yet.
# Until it lands, the `type summary`/`type synthetic` registrations at the bottom
# match no types, so LLDB uses its defaults and nothing here misfires. The
# rendering helpers are already written so they light up the moment those types
# exist; the only pending piece is the exact type names to bind them to.
#
# Every Quilon aggregate lowers to a `{ ptr, i64 }`-ish struct today (an array is
# `{ data, size }`; a Text is UTF-8 bytes `{ data, size }`), which is why, absent
# distinct types, they are indistinguishable to the debugger and cannot be
# auto-formatted by shape alone.

import lldb


def _child(valobj, *names):
    """First present child among `names` (tolerates the field-name churn the
    debug-types work may introduce)."""
    for name in names:
        child = valobj.GetChildMemberWithName(name)
        if child and child.IsValid():
            return child
    return None


def quilon_text_summary(valobj, _internal_dict):
    """Render a Quilon Text ({ data: ptr, size: i64 } of UTF-8 bytes) as a
    quoted string. PENDING (DWARF types): bound only once Text has its own type."""
    try:
        data = _child(valobj, "data", "ptr")
        size = _child(valobj, "size", "len")
        if data is None or size is None:
            return None
        n = size.GetValueAsSigned(0)
        if n <= 0:
            return '""'
        addr = data.GetValueAsUnsigned(0)
        if addr == 0:
            return '""'
        err = lldb.SBError()
        raw = valobj.GetProcess().ReadMemory(addr, min(n, 4096), err)
        if not err.Success() or raw is None:
            return None
        text = bytes(raw).decode("utf-8", "replace")
        return '"{}"'.format(text)
    except Exception:
        return None


def quilon_array_summary(valobj, _internal_dict):
    """Render a Quilon array ({ data: ptr, size: i64 }) with its length.
    PENDING (DWARF types): element rendering needs the element type from DWARF."""
    try:
        size = _child(valobj, "size", "len")
        if size is None:
            return None
        n = size.GetValueAsSigned(0)
        return "array[{}]".format(n)
    except Exception:
        return None


def __lldb_init_module(debugger, _internal_dict):
    # PENDING (DWARF types, not yet merged): bind the summaries above to the
    # concrete type names the compiler's debug-types work will emit for Text,
    # arrays, records and sum types. The names below are placeholders for that
    # work; until those types exist these regex matches bind to nothing, which
    # is intentional and harmless. Update the names (and add record/sum-variant
    # synthetics) when the debug-types build lands.
    commands = [
        r'type summary add -x "^Quilon::Text$" -F quilon.quilon_text_summary',
        r'type summary add -x "^Quilon::Array" -F quilon.quilon_array_summary',
    ]
    for command in commands:
        debugger.HandleCommand(command)
