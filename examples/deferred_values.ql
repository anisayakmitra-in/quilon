~ Concurrency runtime — the `@sleep` leaf IO primitive (see docs/LANGUAGE.md).
~ `@sleep(seconds)` is an effect-only pause: used as a statement it waits right there on
~ the current fiber, then execution continues in program order. It carries no value yet —
~ the deferred-value / overlap story arrives with a value-returning primitive (`@read`).
~ `now()` reads a monotonic clock, so this example VERIFIES the pause actually waited: the
~ elapsed time across the sleep is at least the requested duration. Self-asserting via
~ core.test; exit 0 = pass.

<< core.test
<< core.time

^ = () -> Num => <
  start = now()
  @sleep(0.05)                    ~ pause ~50ms
  ~ A sleep waits AT LEAST its duration, so this bound is deterministic (never flaky).
  assert(now() - start >= 0.05)

  ~ Ordinary code runs after the pause, computing a ready value.
  answer = 6 * 7
  assertEq(answer, 42)

  0
>
