~ core.time — time-related leaf IO primitives. Import with `<< core.time`.
~
~   @sleep(ms)   sleep for `ms` milliseconds, then yield `ms` (Num -> Num).
~
~ `@sleep` is a *deferring* leaf IO primitive (the `@` marker). Calling it does NOT
~ block the caller: it launches the sleep and returns a *deferred* Num immediately, so
~ independent `@sleep` calls overlap. The value is *forced* — the fiber parks until the
~ sleep elapses — only at a strict operation (arithmetic, comparison, output, a native
~ call, the `^` exit). A `< >` block joins any launch it started before it returns.
~
~ Example:
~   << core.time
~   ^ = () -> Num => <
~     a = @sleep(50)          ~ launches, returns immediately
~     b = @sleep(50)          ~ launches too — the two overlap (~50ms, not ~100ms)
~     a + b                   ~ forces both here → 100
~   >
~
~ `@sleep` is compiler-provided (a built-in overload lowered to the runtime scheduler's
~ sleep, like `print` and `write`); this module is its documented home, so a program
~ makes its intent explicit with `<< core.time`. There is no user-declarable `@`
~ primitive — the `@` marker is reserved for the corelib/runtime.
