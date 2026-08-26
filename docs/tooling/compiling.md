# Compiling & running

Source files are **`.qn`**, and the compiler rejects a source named anything else. (Quilon
used `.ql` until 0.9.1; it is CodeQL's extension, so GitHub attributed Quilon programs to
CodeQL. Rename a `.ql` file to `.qn` — nothing else about it changes.)

```bash
quilon check   program.qn   # front-end only (lex + parse + resolve imports + typecheck)
quilon run     program.qn   # front-end, then JIT-execute in-process (exit code = ^'s result)
quilon build   program.qn   # produce a native executable
quilon compile program.qn   # emit LLVM IR → program.ll (for inspection)
```

`quilon build` emits an object file in-process and links it (with the Quilon runtime `libquilon_rt`, which carries the GC) into a native executable:
```bash
quilon build program.qn -o program       # default linker: clang
quilon build program.qn --linker gcc      # gcc also supported (CI checks both)
./program; echo "exit: $?"
```

Add `--debug` (or `-g`) to emit **DWARF debug info** for source-level debugging — a
debugger (`gdb`/`lldb`) can then set breakpoints, step, show backtraces in terms of
`.qn` lines, and **inspect local variables with their Quilon types**:
```bash
quilon build program.qn --debug -o program
llvm-dwarfdump --debug-line program        # lists the .qn file + its line table
llvm-dwarfdump --debug-info program        # shows variables + their debug types
gdb ./program                              # break/step by .qn line, print locals
```
Debug info is opt-in: without `--debug` the binary carries none. It covers line tables,
per-function scopes, and **locals, parameters, and debug types**. Every `=`/`:=` local and
parameter is emitted with its type, and nested `{ }` blocks and closures get their own
lexical scopes. Each Quilon type gets a distinct DWARF entry: `Num`/`Bool` as base types,
and `Text`, arrays (`[]T`), records, and sum types as distinctly-named composites. A
debugger therefore tells them apart despite their shared `{ptr, i64}`-ish machine shape.
Line info is multi-file: a function from an imported module (`<<`) — corelib included — is
attributed to its OWN source, so a debugger steps into it. The entry frame reads `^` (the
generated C `main` shim is named for the entry point and marked artificial). The leaf `@`
primitives and the inert built-in placeholders (`print`/`now`/…) lower to intrinsics and
emit no subprogram, so a debugger steps over them.

(During development, prefix any command with `cargo run --`, e.g. `cargo run -- run program.qn`.)
