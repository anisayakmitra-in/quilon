# Quilon value formatters for LLDB / CodeLLDB, imported by the VS Code extension.
# Bound against the DWARF composite types the compiler emits:
#   Text -> struct { char* data; i64 byte_len }   (NUL-terminated UTF-8)
#   []T  -> struct { T*    data; i64 size }        (name is "[]Num", "[][]Text", …)

import lldb

# Element children beyond this cap are hidden from the default expansion (still
# reachable by explicit index); the overflow count shows in the summary.
ELEMENT_CAP = 200
# Guards a corrupt byte_len from driving an unbounded read.
TEXT_BYTE_CAP = 4096


def text_summary(valobj, _internal_dict):
    raw = valobj.GetNonSyntheticValue()
    data = raw.GetChildMemberWithName("data")
    byte_len = raw.GetChildMemberWithName("byte_len")
    if not data.IsValid() or not byte_len.IsValid():
        return None
    n = byte_len.GetValueAsSigned(0)
    addr = data.GetValueAsUnsigned(0)
    if addr == 0 or n <= 0:
        return '""'
    err = lldb.SBError()
    buf = valobj.GetProcess().ReadMemory(addr, min(n, TEXT_BYTE_CAP), err)
    if not err.Success() or buf is None:
        return None
    text = bytes(buf).decode("utf-8", "replace")
    if n > TEXT_BYTE_CAP:
        return '"{}" (…{} more bytes)'.format(text, n - TEXT_BYTE_CAP)
    return '"{}"'.format(text)


class TextChildrenProvider:
    """A Text is a leaf: show its string, not the {data, byte_len} struct."""

    def __init__(self, _valobj, _internal_dict):
        pass

    def num_children(self):
        return 0

    def get_child_index(self, _name):
        return -1

    def get_child_at_index(self, _index):
        return None

    def update(self):
        return False


def array_summary(valobj, _internal_dict):
    raw = valobj.GetNonSyntheticValue()
    size = raw.GetChildMemberWithName("size")
    n = size.GetValueAsSigned(0) if size.IsValid() else 0
    name = raw.GetType().GetName() or "[]?"
    if n > ELEMENT_CAP:
        return "{} (size={}, first {} shown, …{} more)".format(name, n, ELEMENT_CAP, n - ELEMENT_CAP)
    return "{} (size={})".format(name, n)


class ArrayChildrenProvider:
    """Expose `data[i]` as `[i]` children, each typed as the element type so
    Text/nested-array elements format too. The default expansion is capped at
    ELEMENT_CAP, but an explicit `arr[i]` past the cap still resolves."""

    def __init__(self, valobj, _internal_dict):
        self.valobj = valobj
        self.data = None
        self.size = 0
        self.elem_type = None
        self.elem_size = 0

    def update(self):
        self.data = self.valobj.GetChildMemberWithName("data")
        size = self.valobj.GetChildMemberWithName("size")
        n = size.GetValueAsSigned(0) if size.IsValid() else 0
        if self.data.IsValid():
            self.elem_type = self.data.GetType().GetPointeeType()
            self.elem_size = self.elem_type.GetByteSize()
        else:
            self.elem_type = None
            self.elem_size = 0
        self.size = max(0, n) if self.elem_size else 0
        return False

    def num_children(self):
        return min(self.size, ELEMENT_CAP)

    def get_child_index(self, name):
        try:
            return int(name.strip("[]"))
        except ValueError:
            return -1

    def get_child_at_index(self, index):
        if index < 0 or index >= self.size or self.elem_size == 0:
            return None
        addr = self.data.GetValueAsUnsigned(0) + index * self.elem_size
        return self.valobj.CreateValueFromAddress("[{}]".format(index), addr, self.elem_type)


def __lldb_init_module(debugger, _internal_dict):
    # Text binds by exact name; arrays by the `[]…` name prefix, which also
    # covers nested arrays (`[][]Text` starts with `[]`), so each element of a
    # nested array is handled by the same provider.
    commands = [
        'type summary add -F quilon.text_summary Text',
        'type synthetic add -l quilon.TextChildrenProvider Text',
        r'type summary add -x "^\[\]" -F quilon.array_summary',
        r'type synthetic add -x "^\[\]" -l quilon.ArrayChildrenProvider',
    ]
    for command in commands:
        debugger.HandleCommand(command)
