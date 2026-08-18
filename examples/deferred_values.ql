~ Concurrency runtime — the `@sleep` leaf IO primitive (see docs/LANGUAGE.md).
~ `@sleep(secs)` is an effect-only pause: used as a statement it waits right there on the
~ current fiber, then execution continues in program order. It carries no value yet — the
~ deferred-value / overlap story arrives with a value-returning primitive (`@read`). This
~ example asserts CORRECTNESS: the code runs through the pauses and computes the right
~ ready result. Self-asserting via core.test; exit 0 = pass.

<< core.test
<< core.time

~ Pause twice (sequential), then compute a ready value.
answerAfterPauses = () -> Num => <
  @sleep(0.02)      ~ pause ~20ms
  @sleep(0.02)      ~ pause again — waits here too
  6 * 7             ~ ready arithmetic, evaluated after the pauses
>

^ = () -> Num => <
  assertEq(answerAfterPauses(), 42)
  0
>
