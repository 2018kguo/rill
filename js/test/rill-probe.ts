// The drop-in's DOM for each argument, next to Streamdown's (`\n` escapes allowed).
import { mount } from "./dom.ts";
const { Streamdown } = await import("streamdown");
const ours = await import("../src/streamdown.ts");
const theirs = mount(Streamdown);
const mine = mount(ours.Streamdown);
for (const raw of process.argv.slice(2)) {
  const text = raw.replaceAll("\\n", "\n");
  const a = await theirs.render(text);
  const b = await mine.render(text);
  console.log(JSON.stringify(text), a === b ? "same" : "DIFFERENT");
  console.log(" ", b.replace(/ class="[^"]*"| data-streamdown="[^"]*"/g, "").split("\n").join("⏎"));
}
