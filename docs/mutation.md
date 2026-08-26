# Mutation: in-place field writes & setters

Mutability is decided by the binding operator, and governs in-place mutation as well as
reassignment:

- An `=`-bound instance is **immutable**: no field writes, and calling a setter method on it
  is a compile error.
- A `:=`-bound instance is **mutable**: a direct field write `obj.field := value` (in place,
  no re-allocation) and any **setter** method.
- One exception, by type rather than by binding: a [`Site`](functions/site.md) is
  read-only — a location is a value, not a variable — so writing one of its fields is an
  error even through a `:=` binding.

```quilon
Counter = {
  value :: Num,
  bump = (by :: Num) => it.value := it.value + by   ~ setter: writes `it.value := …`
}

c := Counter { value = 30 }   ~ `:=` -> mutable
c.bump(5)                      ~ setter mutates in place -> value = 35
c.value := c.value + 7         ~ direct field write    -> value = 42
```

A method is a **setter** iff its body writes `it.field := …` (or calls another setter on
`it`) — no marker; the `:=` is the signal. A setter call requires a `:=` receiver:

```quilon
c = Counter { value = 30 }   ~ `=` -> immutable
c.value := 99                 ~ error: cannot write a field of immutable `c`
c.bump(5)                     ~ error: cannot call mutating method `bump` on immutable `c`
```

Getter methods carry no `it.field := …`, so they are callable on `=` instances too. (See
`examples/mutation.qn`.)
