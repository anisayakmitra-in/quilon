# Functions

```quilon
greet  = => "Hello!"                       ~ no params
double = x => x * 2                        ~ one param, no parens
add    = (a, b) => a + b                   ~ multiple params
typed  = (a :: Num, b :: Num) -> Num => a + b
```
Multi-statement bodies use `< >` blocks (the last expression is the value):
```quilon
compute = x => <
  doubled = x * 2
  doubled * doubled
>
```
Functions may recurse; a recursive function needs a `-> Type` annotation:
```quilon
factorial = n -> Num => n == 0 ? 1 : n * factorial(n - 1)
```
(See `examples/factorial.qn`, `examples/fibonacci.qn`.)

## Names resolve top to bottom

A call may only name something **already defined above it** — there is no hoisting. A
definition is in scope for its own body (so a function may recurse) and for everything
that follows it, but not for anything before it:
```quilon
^ = () -> Num => later()   ~ error: Undefined variable 'later'
later = () -> Num => 7
```
This holds for overload-set members too, which report the situation by name:
```quilon
h = () -> Text => g(1)     ~ error: cannot call 'g' before its definition
g = (n :: Num) -> Text => "a"
g = (t :: Text) -> Text => "b"
```
So **mutual recursion between top-level functions is not expressible**: whichever of the
pair comes first would have to call the other before it exists. Self-recursion is
unaffected, including a recursive overload member calling itself. Restructure a mutual
pair into one self-recursive function.
