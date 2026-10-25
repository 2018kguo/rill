import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import { Chat } from "./chat";

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <h1>rill on Vite</h1>
    <Chat />
  </StrictMode>
);
