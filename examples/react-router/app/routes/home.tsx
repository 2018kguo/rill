import { Chat } from "../chat";

export const meta = () => [{ title: "rill on React Router" }];

export default function Home() {
  return (
    <main>
      <h1>rill on React Router</h1>
      <Chat />
    </main>
  );
}
