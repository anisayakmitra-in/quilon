~ Blocks `< ... >` evaluate to their last expression. Numbers are one unified `Num`.
~ `%` is the f64 remainder: it works on fractional operands and the result takes
~ the DIVIDEND's sign (like C fmod / Rust %).
<< core.test

^ = () -> Num => <
  assertEq(7 % 3, 1)
  assertEq(7.5 % 2, 1.5)            ~ fractional operands work
  assertEq(10 % 2.5, 0)
  assertEq((0 - 7) % 3, 0 - 1)      ~ sign follows the dividend...
  assertEq(7 % (0 - 3), 1)          ~ ...not the divisor

  a = 5
  b = 7
  a + b          ~ exit code 12
>
