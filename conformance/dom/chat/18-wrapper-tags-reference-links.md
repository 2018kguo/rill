## Paths, with [the reference][ref]

Listing 7-3 calls the function by its [absolute path][pub]<!-- ignore -->:

<Listing number="7-3" file-name="src/lib.rs" caption="Calling `add_to_waitlist` with absolute and relative paths">

```rust
pub fn eat_at_restaurant() {
    crate::front_of_house::hosting::add_to_waitlist();
}
```

</Listing>

The first call uses an absolute path.

Text [pub].

<Listing a="1">

Para

</Listing>

After.

<Listing number="7-4">

A listing that holds a paragraph.

</Listing>
<Listing number="7-5">

And one left open, with everything after it inside.

- a list
- of items

[ref]: https://doc.rust-lang.org/reference/
[pub]: https://doc.rust-lang.org/book/ch07-03-paths-for-referring-to-an-item-in-the-module-tree.html
