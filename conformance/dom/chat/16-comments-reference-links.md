<!-- a comment before anything else -->

## Comments between blocks

The [Rust Book][book] keeps regeneration notes in HTML comments, which sanitize
removes; the line endings on both sides of each stay.

<!-- manual-regeneration
cd listings/ch03-common-programming-concepts/no-listing-32-loop
cargo run
CTRL-C
-->

```console
$ cargo run
again!
^Cagain!
```

Two comments in a row:

<!-- one -->

<!-- two -->

| Column | Value |
| --- | --- |
| a | 1 |

<!-- after a table -->

Text right before a definition-only piece, then a [link][docs].

[book]: https://doc.rust-lang.org/book/
[docs]: https://docs.rs

<!-- the last block -->
