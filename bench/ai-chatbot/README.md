# Vercel's ai-chatbot: Streamdown vs rill

The README's "Real app" numbers: [vercel/ai-chatbot](https://github.com/vercel/ai-chatbot)
(Next.js 16, React 19, its `MessageResponse` on Streamdown with the `cjk`, `code`, `math`
and `mermaid` plugins), built for production twice, once as it is and once with rill's
import, and traced in Chromium.

## What changes in the template

- `template.patch`, in both builds: Streamdown 2.3.0 to 2.7.1 and its plugins to their
  current versions (rill is a drop-in for 2.7.1; Streamdown 2.7.1's stricter prop types
  need one `Omit` in `reasoning.tsx`), and a local mode without keys: the template's own
  mock model (`PLAYWRIGHT=True`) for every model id, streaming `answer.md` when the prompt
  says "benchmark", Auth.js cookies over `http://localhost`, and no BotID check (it only
  classifies requests on Vercel).
- `rill.patch`, in the rill build only: the import, in the two files that render
  Markdown. No config.

```diff
-import { Streamdown } from "streamdown";
+import { Streamdown } from "@2018kguo/rill/streamdown";
```

## Run it

```sh
sh examples/pack.sh --build                      # from the repository root
cd bench/ai-chatbot && npm install
createdb aichatbot_bench
sh setup.sh ~/ai-chatbot-bench postgres://localhost/aichatbot_bench
(cd ~/ai-chatbot-bench/streamdown && npx next start -p 4801) &
(cd ~/ai-chatbot-bench/rill && npx next start -p 4802) &
node bench.mjs --app streamdown=http://localhost:4801 --app rill=http://localhost:4802
```

Defaults: both scenarios, 4× and 10× CPU throttle, 5 runs after 1 warm-up per engine,
interleaved (alternating which goes first), each in a fresh browser context with a new
guest session, on a 412×915 screen at 2.625×. Results go to `results.json`.

## Scenarios

- **open**: open a chat of 18 long answers (`seed.mjs`: bench/browser's chat, 239 KB of
  Markdown with code, tables and lists), from navigation (the app's code already cached,
  as for a returning visitor) to settled. The template fetches the messages after the
  page loads; "after data" is the part from that response to settled.
- **stream**: in a new chat, press Enter on a prompt; the mock model streams `answer.md`
  (8 KB: five code blocks, three tables, a 30-item list) at 2,400 characters a second.
  The stream itself takes about 3.4 s, so wall time is mostly the stream's; what differs
  is the main thread's work and how much it blocks.

"Settled" is bench/browser's done rule applied to the page: the content is all there (18
answers, or the answer's last sentence), the DOM has not changed for 600 ms with no frame
gap over 100 ms, and no code line is left unhighlighted. A run ends at the end of the
first paint commit after the last DOM change. Every duration comes from the Chrome trace
(`@paulirish/trace_engine`) or Long Animation Frames entries; main-thread time is split
as in bench/browser, and worker time (rill highlights in a worker) is reported apart.

After each run the harness hashes the settled answers' HTML (each
`[data-testid="message-content"]`); the report ends with the hashes per scenario and
engine, which are the same for both.
