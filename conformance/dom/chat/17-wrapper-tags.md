## Listings in wrapper tags

The Rust Book wraps listings in a tag sanitize removes, keeping what is inside:

<Listing number="6-5" caption="A function that uses a `match` expression on an `Option<i32>`">

```rust
fn plus_one(x: Option<i32>) -> Option<i32> {
    match x {
        None => None,
        Some(i) => Some(i + 1),
    }
}
```

</Listing>

> <Listing file-name="src/main.rs">
>
> <!-- CAN'T EXTRACT -->
>
> ```rust
> struct User {
>     username: &str,
> }
> ```
>
> </Listing>
>
> The compiler will complain.

An inline <note>custom tag</note> and <aside-x>one parse5 knows nothing of</aside-x>.

- item <thinking>with **bold** inside</thinking>

<Listing number="10-6" file-name="src/main.rs" caption="Left open: everything after it is inside">

```rust
struct Point<T> {
    x: T,
}
```

| Type | Size |
| --- | --- |
| `u8` | 1 |

The end.
