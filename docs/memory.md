# Memory

Quilon uses a **conservative garbage collector** (Boehm). Heap values (`Text`, etc.) are GC-managed — there is no manual free. The collector is **linked statically** into every binary, so a compiled program carries its own GC and needs nothing installed to run.
