// Prints Streamdown's DOM (as the tests serialize it) for each argument.
import { mount } from "./dom.ts";
const { Streamdown } = await import("streamdown");
const sd = mount(Streamdown);
for (const arg of process.argv.slice(2)) {
  const text = JSON.parse(arg);
  const out = await sd.render(text, { isAnimating: false, controls: false });
  console.log("=== " + JSON.stringify(text) + "\n" + out);
}
await sd.unmount();
