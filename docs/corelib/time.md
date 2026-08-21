# `core.time` — Time

Import with `<< core.time`. See the [Standard library index](../LANGUAGE.md#standard-library) and `examples/sleep.ql`.

`core.time` provides two things: the [`@sleep`](../LANGUAGE.md#concurrency--colorless-implicit-futures--in-progress) leaf IO primitive (a pause) and the monotonic `now()` clock. Both are compiler-lowered to runtime intrinsics.

| Function | Effect |
|----------|--------|
| `@sleep(seconds :: Num) -> $` | Pause the current fiber for `seconds` seconds (a fractional `Num`), then continue. Effect-only (`-> $`): used as a statement it **waits right there** on the current fiber, then execution continues in program order. It carries no value, so nothing defers or forces. A [leaf IO primitive](../LANGUAGE.md#concurrency--colorless-implicit-futures--in-progress) (the `@` marker). |
| `now() -> Num` | Read a **monotonic** clock (seconds as a fractional `Num`). Only *differences* between two readings are meaningful — that is exactly what measures an elapsed duration. A plain (non-`@`) primitive: reading the clock is instant and never parks. |

```quilon
<< core.time

^ = () -> Num => <
  start = now()
  @sleep(0.05)            ~ pause ~50ms, then continue
  now() - start >= 0.05 ? 6 * 7 : 0   ~ the sleep really waited → 42
>
```

Running the entry on the fiber scheduler is what lets `@sleep` park; `^` and any helper it
calls carry no marker, no `async`, no `await` — only the leaf `@sleep` is marked. See the
[Concurrency model](../LANGUAGE.md#concurrency--colorless-implicit-futures--in-progress) for
how `@` leaf primitives, deferred values, and force-at-strict-op fit together. (See `examples/sleep.ql`.)
