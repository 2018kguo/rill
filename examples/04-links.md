Autolinks: https://example.com/path?q=1&r=2, www.example.org/a_(b), user@example.com.

Trailing punctuation: (https://example.com/x). See https://example.com/a\_b_ too.

[Inline](https://example.com "title") and ![image](https://example.com/i.png "t").

[ref] and [ref][] and [Ref][REF] and [nope][missing] and [ref][.

[ref]: https://example.com/ref "Ref title"

[www.inside.com] inside brackets stays a transform link.

<https://angle.example.com> and <mailto:a@b.co>.

[javascript](javascript:alert(1)) is blocked; ![data](data:image/png;base64,AAAA) is allowed.

Footnote call[^note] and another[^note].

[^note]: The footnote, with **emphasis**.
