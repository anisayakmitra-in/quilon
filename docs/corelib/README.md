# Corelib

The corelib — Quilon's standard library — ships with the compiler; import a module with
`<< core.<module>`. Each has its own API reference under [`docs/corelib/`](./):
signatures, behavior, and a small example per function.

| Module | Import | What it gives you |
|--------|--------|-------------------|
| [`core.io`](io.md) | `<< core.io` | Output to file descriptors and stdin: `print` / `eprint` / `write`, the `stdout` / `stderr` descriptors, and the deferred `@readStdin` line read. |
| [`core.test`](test.md) | `<< core.test` | In-language assertions for self-verifying programs, reporting the caller's `file:line:column`: `assert` (+ `AssertOpts`) / `assertEq` / `assertNotEq` / `assertOk` / `assertNotOk` / `failAt` (fail → exit 101). |
| [`core.cli`](cli.md) | `<< core.cli` | Pipe-friendly helpers over the entry point's `args` / `env`: `getEnv` / `hasFlag` / `getOpt`. |
| [`core.time`](time.md) | `<< core.time` | Time primitives: the `@sleep` pause and the monotonic `now()` clock. |
| [`core.net`](net.md) | `<< core.net` | Networking: the deferred `@tcpRequest` raw TCP request exchange the HTTP client sits on. |

`Text` and the operators are built-ins and need **no** import. The [concurrency model](../concurrency/README.md) that governs the `@` leaf primitives (`@readStdin`, `@sleep`) is language semantics — see that section.
