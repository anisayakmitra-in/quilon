# Arrays — `[]T`
```quilon
nums  = [1, 2, 3, 4, 5]
count = nums.size      ~ → 5
first = nums[0]        ~ → 1
```
Arrays are `{ ptr, size }` internally. (See `examples/arrays.qn`.)

Indexing is **checked** (fail loud, never silent): an out-of-bounds, negative, or NaN index
is a runtime error naming the read that failed ([shape](../tooling/errors.md)), exit status 1 —
never a raw memory read. A **fractional** in-range index truncates toward zero (`nums[1.7]`
reads `nums[1]`); with one unified `Num`, index arithmetic like `size / 2` legitimately
produces fractions. Use [`at(n)`](#array-methods) for the non-aborting `Ok`/`NotOk` form when
an index might be out of range — see the computed-index case at the end of
`examples/array_methods.qn`.

## Array methods

Arrays carry a set of **built-in, compiler-provided methods**, called with method
syntax (`arr.method(...)`) and freely chainable. The higher-order ones take a **lambda**
(`x => …`, `(a, b) => …`) — an anonymous function literal valid **only** as a direct
argument to one of these methods. The compiler **inlines** the lambda body per element
rather than passing it as a function value (a deliberate specialization — Quilon's
closures are not accepted as higher-order arguments here).

| Method | Result | Notes |
|--------|--------|-------|
| `map(f)` | new `[]R` | element type `R` is `f`'s return type (so `map` may change the element type, e.g. `[]Num → []Text`) |
| `filter(pred)` | new `[]elem` | keeps the elements where `pred` returns `Bool` `true`, in order; `pred` **must** return `Bool` |
| `reduce(init, (acc, x) => …)` | the accumulator | fold-left from `init`; the reducer's result type must match `init`'s type |
| `each(f)` | **the receiver array** | runs `f` for side effects, then returns the array itself, so it chains |
| `find(pred)` | `Ok(elem)` / `NotOk` | the first element satisfying `pred`, absent-safe; `pred` returns `Bool` |
| `at(n :: Num)` | `Ok(elem)` / `NotOk` | non-aborting index — `Ok` in bounds, `NotOk` otherwise (incl. NaN); raw `arr[n]` aborts with a runtime error instead |

```quilon
nums = [1, 2, 3, 4, 5, 6]

total = nums
  .map(x => x * 2)              ~ [2, 4, 6, 8, 10, 12]
  .filter(x => x > 4)           ~ [6, 8, 10, 12]
  .reduce(0, (acc, x) => acc + x)   ~ 36

first = nums.find(x => x > 3) ?  ~ Ok(4)
  | Ok(v)    => v
  | NotOk(_) => 0

third = nums.at(2) ?             ~ Ok(3)
  | Ok(v)    => v
  | NotOk(_) => 0
```

These methods are **reserved on arrays**: a user can define a same-named function/overload
(e.g. a `map` on a `Num`), but on an *array receiver* the built-in always wins — it is
resolved ahead of the overload set. `map`/`reduce`/`find` work over any element type
(e.g. `[]Text`), not just `[]Num`. (See `examples/array_methods.qn`.)

## Array concatenation — `+`

`+` on arrays builds a **new** array (it never mutates an operand), in three forms — each
selected by the **exact** operand types, so there is never any ambiguity:

```quilon
~ concat:  []T + []T -> []T
[1, 2] + [3, 4]          ~ [1, 2, 3, 4]
["a"] + ["b", "c"]       ~ ["a", "b", "c"]

~ append:  []T + T   -> []T   (add one element at the end)
[1, 2] + 3               ~ [1, 2, 3]
["a"] + "b"              ~ ["a", "b"]

~ prepend: T   + []T -> []T   (add one element at the front)
0 + [1, 2]               ~ [0, 1, 2]
```

Both sides must agree on the element type — `[]Num + []Text` (or `[]Num + Text`) is a
type error. The forms are mutually exclusive (an array `[]T` can never equal its own
element `T`), so even nested arrays disambiguate cleanly: `[][]Num + []Num` is an
**append** (the `[]Num` is a single new row → `[][]Num`), while `[][]Num + [][]Num` is a
**concat**. `[]T + []T` is the same as the spread `[<-a, <-b]` and shares its element-copy
lowering, so it is element-repr-correct for `[]Num`, `[]Text`, and nested arrays alike.
(See `examples/array_concat.qn`.)
