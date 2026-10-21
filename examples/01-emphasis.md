# Emphasis edge cases

***strong emph*** and **strong *nested emph* strong** and *emph **nested strong***.

****_***' — two delimiters per step, then the rule of 3 on what remains.

*~_~_ mixes attention and strikethrough: `_` resolves first, so `~` can't cross it.

~~strike *with emph* inside~~ and ~single~ tilde, ~~~not three~~~.

日本語の**強調**です。中文**加粗**测试，**「括号」**也可以。

snake_case_words stay plain, but _this_ is emphasis. a*b*c and a_b_c.

\*escaped\* and \_escaped\_ and `code with *stars*`.
