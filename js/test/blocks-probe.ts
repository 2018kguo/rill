// Streamdown's block split of each argument (`\n` escapes allowed).
import { parseMarkdownIntoBlocks } from "streamdown";
for (const text of process.argv.slice(2)) console.log(JSON.stringify(parseMarkdownIntoBlocks(text.replaceAll("\\n", "\n"))));
