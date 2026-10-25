// The smoke test's Markdown: a settled answer (rendered on the server) and one to stream.
const section = (n: number) => `## Step ${n}

Read the **config**, then call \`load()\` with the [docs](https://example.com/${n}).

\`\`\`ts
export function step${n}(input: string): number {
  // Parse and count.
  return input.split("\\n").length + ${n};
}
\`\`\`

| Option | Default | Notes |
|:--|--:|---|
| \`retries\` | ${n} | *per request* |
| \`timeout\` | ${n * 1000} | ~~seconds~~ ms |

- first, with a [link](https://example.com)
- [x] done
  1. nested
`;

export const HISTORY = `# Settled answer

${[1, 2, 3].map(section).join("\n")}
> A quote with **bold** text.
`;

export const ANSWER = `# Streamed answer

${[4, 5, 6, 7, 8].map(section).join("\n")}
\`\`\`python
def done():
    return "ok"
\`\`\`
`;
