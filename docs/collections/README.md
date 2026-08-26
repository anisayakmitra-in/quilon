# Collections

## Maps

A `Map` is a **built-in parametric collection** — like `[]T`, not a user-defined generic —
written with a **pipe fence** `[|K => V|]` (`=>` reads "maps to"). It is immutable, keyed by
`Num`/`Text`/`Bool` or a **user type** that defines both a `%` hash hook and an `==` member,
and read through `.get` (which returns a `Result` — there is no bracket indexing on a map).
Full reference: [`docs/collections/map.md`](map.md) (and `examples/maps.qn`).

## Sets

A `Set` is a **built-in parametric collection** — like `[]T`, not a user-defined generic —
written with the same **pipe fence** `[|T|]` (which keeps a set literal distinct from an array).
It is immutable, holds unique `Num`/`Text`/`Bool` elements (or a **user type** defining both
a `%` hash hook and an `==` member), and supports set algebra
(`+` union, `-` difference, `+-`/`-+` intersection). Full reference:
[`docs/collections/set.md`](set.md) (and `examples/sets.qn`).
