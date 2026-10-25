// A server component: the chat below renders on the server and hydrates, and the
// drop-in also renders here directly, as Streamdown does.
import { Streamdown } from "@2018kguo/rill/streamdown";

import { Chat } from "./chat";

export default function Page() {
  return (
    <main>
      <h1>rill on Next.js</h1>
      <section data-testid="server">
        <Streamdown>{"Rendered from a **server component**."}</Streamdown>
      </section>
      <Chat />
    </main>
  );
}
