~ core.time — time primitives. `@sleep` is a leaf IO primitive (the `@` marker),
~ compiler-lowered to the runtime scheduler's sleep. Import with `<< core.time`.

~ Pause the current fiber for `secs` seconds (a fractional Num), then continue.
>> @sleep = (secs :: Num) -> $ => $
