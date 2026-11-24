// Streamdown's DOM for each argument (remend applied as Streamdown does).
import { mount } from "./dom.ts";
const { Streamdown } = await import("streamdown");
const theirs = mount(Streamdown);
for (const text of process.argv.slice(2)) {
  const html = await theirs.render(text.replaceAll("\\n", "\n"));
  console.log(JSON.stringify(text), "\n ", html, "\n");
}
