// Prints remark's MDAST and rill's side by side: node diff.mjs is the check, this is the lens.
//   node probe.mjs '"[^1]:\n\n"'      (a JSON string)
import { spawnSync } from "node:child_process";

import { processor } from "./pipeline.mjs";

const source = JSON.parse(process.argv[2]);
const bytes = new TextEncoder().encode(source);
const binary = new URL("../target/release/examples/stdio", import.meta.url).pathname;
const run = spawnSync(binary, { input: Buffer.concat([Buffer.from(`${bytes.length}\n`), bytes]) });
const rill = JSON.parse(run.stdout.toString());

function lines(node, depth = 0, out = []) {
  const p = node.position;
  const at = p ? `${p.start.line}:${p.start.column}-${p.end.line}:${p.end.column}` : "-";
  out.push(`${"  ".repeat(depth)}${node.type} ${at} ${node.value === undefined ? "" : JSON.stringify(node.value)}`);
  for (const child of node.children ?? []) lines(child, depth + 1, out);
  return out;
}
const left = lines(processor.parse(source));
const right = lines(rill.mdast);
const width = Math.max(...left.map((line) => line.length), 10) + 2;
console.log(`${"remark".padEnd(width)}rill`);
for (let index = 0; index < Math.max(left.length, right.length); index += 1) {
  const a = left[index] ?? "";
  const b = right[index] ?? "";
  console.log(`${a.padEnd(width)}${b}${a === b ? "" : "   <<"}`);
}
