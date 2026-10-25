// SQL for the benchmark's long existing chat: bench/browser's 18 long answers (the same
// chat as the README's "open a chat" row), each after its question, in one public chat
// owned by a seeded user. `node seed.mjs | psql "$POSTGRES_URL"`.
import { readFileSync } from "node:fs";

export const CHAT_ID = "00000000-0000-4000-8000-000000000018";
const USER_ID = "00000000-0000-4000-8000-0000000000aa";
const turns = JSON.parse(readFileSync(new URL("../browser/stage/chat.json", import.meta.url), "utf8"));

const literal = (value) => `'${value.replaceAll("'", "''")}'`;
const json = (value) => `${literal(JSON.stringify(value))}::json`;
const lines = [
  "begin;",
  `delete from "Vote_v2" where "chatId" = '${CHAT_ID}';`,
  `delete from "Message_v2" where "chatId" = '${CHAT_ID}';`,
  `delete from "Chat" where id = '${CHAT_ID}';`,
  `insert into "User" (id, email) values ('${USER_ID}', 'bench@example.com') on conflict (id) do nothing;`,
  `insert into "Chat" (id, "createdAt", title, "userId", visibility) values ('${CHAT_ID}', '2026-01-01T00:00:00Z', 'Long chat', '${USER_ID}', 'public');`,
];
let second = 0;
const at = () => literal(new Date(Date.UTC(2026, 0, 1, 0, 0, (second += 1))).toISOString());
for (const { question, answer } of turns) {
  for (const [role, text] of [["user", question], ["assistant", answer]]) {
    lines.push(
      `insert into "Message_v2" ("chatId", role, parts, attachments, "createdAt") values ('${CHAT_ID}', '${role}', ${json([{ type: "text", text }])}, '[]'::json, ${at()});`
    );
  }
}
lines.push("commit;");
if (import.meta.url === `file://${process.argv[1]}`) console.log(lines.join("\n"));
