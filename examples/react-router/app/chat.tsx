import { code } from "@2018kguo/rill/code";
import { blockRenders, highlightCounts, Streamdown } from "@2018kguo/rill/streamdown";
import { loadedRill } from "@2018kguo/rill";
import { useEffect, useState } from "react";

import { ANSWER, HISTORY } from "./content";

export function Chat() {
  const [text, setText] = useState("");
  const [streaming, setStreaming] = useState(false);

  useEffect(() => {
    // For the smoke test: whether rill loaded and highlighted.
    Object.assign(window, { rillStatus: () => ({ loaded: loadedRill() !== null, ...highlightCounts(), blocks: { ...blockRenders } }) });
  }, []);

  const stream = () => {
    setStreaming(true);
    let at = 0;
    const timer = setInterval(() => {
      at = Math.min(ANSWER.length, at + 40);
      setText(ANSWER.slice(0, at));
      if (at === ANSWER.length) {
        clearInterval(timer);
        setStreaming(false);
      }
    }, 16);
  };

  return (
    <>
      <section data-testid="history">
        <Streamdown plugins={{ code }}>{HISTORY}</Streamdown>
      </section>
      <button type="button" data-testid="stream" onClick={stream} disabled={streaming}>
        Stream an answer
      </button>
      <section data-testid="answer" data-done={!streaming && text.length > 0}>
        <Streamdown plugins={{ code }} isAnimating={streaming}>
          {text}
        </Streamdown>
      </section>
    </>
  );
}
