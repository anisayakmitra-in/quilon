# Iteration — array methods + recursion
Quilon has **no `for`/`while` loop**. A collection is iterated with the built-in
[array methods](../types/arrays.md#array-methods): `.each` runs a body for its side effects (the direct
replacement for a side-effecting loop), and `.map`/`.filter`/`.reduce` transform or fold
without any mutable accumulator. Each takes a lambda the compiler inlines per element:
```quilon
nums = [1, 2, 3]
nums.each(n => print(n))              ~ side effects; returns the receiver (chainable)

sum = nums
  .map(n => n * 2)                    ~ [2, 4, 6]
  .reduce(0, (acc, n) => acc + n)     ~ 12
```
When iteration doesn't fit a method, use **recursion**: a self-tail-call is
[guaranteed to be lowered to a loop](../functions/closures.md#tail-self-recursion-is-optimized-to-a-loop-guaranteed),
so even deep recursion runs in constant stack. (See `examples/iteration.qn`.)
