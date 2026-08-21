~ Deferred values — `@read` (see docs/LANGUAGE.md, "Deferred values").
~ `@read()` is a value-returning leaf IO primitive: it LAUNCHES a stdin line read in the
~ background and hands back a DEFERRED Text immediately. The value threads through the `line`
~ binding untouched (promise pipelining); the fiber only WAITS (forces) at the first strict
~ operation that reads its bytes — here the `==` inside `assertEq`.
~
~ This example needs input, so the no-input examples gate does not run it (it is compile-only
~ there). Run it as documented; it self-asserts and exits 0 for that input:
~   echo hello | cargo run -- run examples/deferred_read.ql

<< core.io
<< core.test

^ = () -> Num => <
  line = @read()              ~ launches the read, returns a deferred Text (no wait here)
  assertEq(line, "hello")     ~ the comparison FORCES the value: waits, then reads the bytes
  0
>
