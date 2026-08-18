~ Deferred values & implicit concurrency — Quilon's colorless implicit-futures model.
~ See docs/LANGUAGE.md "Concurrency — colorless implicit futures".
~
~ `@sleep` is a deferring leaf IO primitive: calling it launches a sleep and returns a
~ *deferred* Num immediately, without blocking. Independent launches therefore overlap.
~ A deferred value is *forced* only at a strict operation (here, arithmetic); threading
~ it through bindings stays lazy. This example asserts CORRECTNESS (the deferred values
~ thread through and force to the right results) — an exit-code test cannot assert the
~ wall-clock overlap, but the same launches run concurrently under the scheduler.

<< core.test
<< core.time

~ Two independent launches: both start before either is forced, so they overlap.
overlap = () -> Num => <
  a = @sleep(10)          ~ launches, returns a deferred Num
  b = @sleep(20)          ~ launches too — overlaps a
  a + b                   ~ forced here at the arithmetic frontier → 30
>

~ A deferred value threaded through several bindings and forced at the end. Reading a
~ deferred binding is lazy; the force is memoized, so using `a` more than once is fine.
threaded = () -> Num => <
  a = @sleep(5)
  b = a                   ~ still deferred — a plain rebind does not force
  c = b                   ~ still deferred
  c + a - @sleep(4)       ~ forces a, c, and the third launch → 5 + 5 - 4 = 6
>

^ = () -> Num => <
  assertEq(overlap(), 30)
  assertEq(threaded(), 6)

  ~ A bound-but-never-used launch is still joined by its enclosing `< >` block, so a
  ~ launched effect never silently vanishes.
  unused = @sleep(0)

  0
>
