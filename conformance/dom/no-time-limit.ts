// Shiki stops tokenizing a line once it has taken tokenizeTimeLimit (500 ms by default,
// and @streamdown/code passes no option) and leaves the rest of the line in the scope
// it had reached. On a slow or busy machine a cold grammar's first lines can take that
// long, so Streamdown's side would depend on the machine: on GitHub's runners its
// light-theme pass over a line of the Deno README stopped early and its dark-theme pass
// did not. rill-highlight always tokenizes to the end; so does Shiki on this page.
import { createHighlighter } from "shiki";
import { createJavaScriptRegexEngine } from "shiki/engine/javascript";

const highlighter = await createHighlighter({
  themes: [],
  langs: ["json"],
  engine: createJavaScriptRegexEngine(),
});
// vscode-textmate's Grammar class, shared with every highlighter on the page.
const grammar = Object.getPrototypeOf(highlighter.getLanguage("json"));
highlighter.dispose();
for (const name of ["tokenizeLine", "tokenizeLine2"]) {
  const tokenize = grammar[name];
  if (typeof tokenize !== "function") throw new Error(`no Grammar.${name} to patch`);
  grammar[name] = function (this: unknown, lineText: unknown, prevState: unknown) {
    return tokenize.call(this, lineText, prevState, 0);
  };
}
