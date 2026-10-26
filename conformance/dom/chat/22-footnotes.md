## Why Rust's ownership model matters

Rust manages memory through ownership[^own], a set of rules the compiler checks at
compile time. Nothing about it costs anything while the program runs[^zero].

Each value has one owner. When the owner goes out of scope, the value is dropped[^own].
Borrowing lets other code read a value without taking it[^borrow].

| Concept | Checked | Cost |
|---|---|---|
| Ownership[^own] | compile time | none |
| Borrowing | compile time | none[^zero] |

- Moves transfer ownership[^move]
- Copies duplicate plain data
- References borrow[^borrow]

A label nobody defines stays text: [^missing].

```rust
let s = String::from("hello"); // [^not-a-call]
let t = s;
```

[^own]: The owner is the variable binding that holds the value.
[^zero]: Zero-cost: the checks happen only in the compiler.

Moving a `String` copies its pointer, length and capacity, not the heap data.

[^borrow]: A borrow is a reference, `&T` or `&mut T`.

    A mutable borrow is exclusive: while it lives, nothing else reads the value.

[^move]: See also [the book][book], and the note on owners[^own].

[book]: https://doc.rust-lang.org/book/ch04-01-what-is-ownership.html
