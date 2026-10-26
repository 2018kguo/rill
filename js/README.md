# @2018kguo/rill

**A drop-in replacement for Streamdown that's much faster. Same DOM, byte for byte.**

```sh
npm install @2018kguo/rill
```

```diff
- import { Streamdown } from "streamdown";
+ import { Streamdown } from "@2018kguo/rill/streamdown";
```

With `@streamdown/code`, change that import too, and the page stops shipping Shiki:

```diff
- import { code } from "@streamdown/code";
+ import { code } from "@2018kguo/rill/code";
```

Same component, same props, same page. On a throttled phone profile (Chromium, CPU
throttled 10×), Streamdown 2.7.1 vs rill 0.4.0:

| Scenario | Finished sooner | Less main-thread JavaScript |
|---|---:|---:|
| Open a chat of 18 long answers | 4.4× | 6.6× |
| Open a code-heavy chat | 8.2× | 11.9× |
| Stream a 56 KB answer | 5.2× | 13.7× |

- [How it works](https://github.com/2018kguo/rill/blob/main/docs/how-it-works.md)
- [Benchmarks](https://github.com/2018kguo/rill/blob/main/docs/benchmarks.md), with raw results and how to rerun them
- [Reference](https://github.com/2018kguo/rill/blob/main/docs/reference.md): setup per bundler, API, plugins, fallbacks, custom builds

MIT. Source: [github.com/2018kguo/rill](https://github.com/2018kguo/rill)
