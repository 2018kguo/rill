// Streamdown 2.7.1's `animated` for the drop-in: its timeline (lib/animate.ts
// `createAnimateTimeline`, which it does not export), shared by a message's blocks, and
// each block's state. rill's blocks run the plugin's pass in Rust (crates/rill/src/
// animate.rs); a block left to Streamdown's Block runs Streamdown's own plugin on the
// same timeline, so the delays cascade across both as they would in Streamdown.
import { createAnimatePlugin, type AnimateOptions } from "streamdown";
import type { Pluggable } from "unified";

import type { AnimatePass } from "./core.ts";

type AnimatePlugin = ReturnType<typeof createAnimatePlugin>;

/** Streamdown's `AnimateTimeline`. */
export type Timeline = {
  beginPass(now: number): void;
  commitPass(): void;
  mark(): number;
  now(): number;
  rewind(mark: number): void;
  take(wordCount: number, stagger: number, now: number): { baseDelay: number; step: number };
};

const MAX_ANIMATION_BACKLOG_MS = 320;
const MIN_STAGGER_STEP_MS = 4;

const now = (): number => (typeof performance === "undefined" ? Date.now() : performance.now());

/** lib/animate.ts `createAnimateTimeline`, line for line. */
export function createTimeline(maxBacklogMs?: number): Timeline {
  const maxBacklog = maxBacklogMs ?? MAX_ANIMATION_BACKLOG_MS;
  let committedNextStartAt = 0;
  let passNextStartAt = 0;
  return {
    now,
    beginPass(at) {
      passNextStartAt = Math.min(Math.max(committedNextStartAt, at), at + maxBacklog);
    },
    mark: () => passNextStartAt,
    rewind(mark) {
      passNextStartAt = mark;
    },
    take(wordCount, stagger, at) {
      if (wordCount <= 0) return { baseDelay: 0, step: Math.max(0, stagger) };
      const idealStep = Math.max(0, stagger);
      const minStep = idealStep === 0 ? 0 : Math.min(idealStep, MIN_STAGGER_STEP_MS);
      const startAt = Math.max(passNextStartAt, at);
      const budgetEnd = at + maxBacklog;
      const idealLast = startAt + Math.max(0, wordCount - 1) * idealStep;
      let step = idealStep;
      if (idealLast > budgetEnd && wordCount > 1) {
        step = startAt < budgetEnd ? Math.max(minStep, (budgetEnd - startAt) / (wordCount - 1)) : minStep;
      }
      const baseDelay = Math.max(0, Math.round(startAt - at));
      passNextStartAt = startAt + wordCount * step;
      return { baseDelay, step };
    },
    commitPass() {
      committedNextStartAt = passNextStartAt;
    },
  };
}

/** What Streamdown's plugin keeps per block, for rill's pass. */
export type BlockAnimation = {
  /** Units of text shown at the last commit (`committedCharCount`). */
  committed: number;
  /** Units the last pass rendered (`lastRenderCharCount`). */
  last: number;
  /** The timeline's cursor at this commit's first pass (`pendingMark`). */
  mark: number | null;
  /** Streamdown's plugin on the same timeline, for when Streamdown's Block renders it. */
  plugin: AnimatePlugin | null;
  /** Streamdown's Block rendered it while animated: it keeps rendering it (switching
   * would remount its elements, which Streamdown reuses). */
  streamdown: boolean;
  /** Its rehype plugins, by the list they extend. */
  lists: WeakMap<object, Pluggable[]>;
};

/** A message's animation: `animated`'s options, the timeline, and each block's state. */
export type Animation = {
  key: string;
  options: AnimateOptions;
  /** Options rill's pass reproduces: an `animation` and `easing` React gets as style
   * values as they are (Streamdown's style objects). */
  plain: boolean;
  timeline: Timeline;
  blocks: BlockAnimation[];
};

// lib/animate.ts: values the style parser returns unchanged.
const plainStyleValue = /^[^\s;:'"\\/](?:[^;:'"\\/\r\n]*[^\s;:'"\\/])?$/;

/** `animated`'s key, as Streamdown keys its timeline. */
export const animatedKey = (animated: boolean | AnimateOptions | undefined): string =>
  animated === true ? "true" : animated ? JSON.stringify(animated) : "";

export function newAnimation(key: string, animated: boolean | AnimateOptions): Animation {
  const options = key === "true" ? {} : (animated as AnimateOptions);
  const animation = options.animation ?? "fadeIn";
  const easing = options.easing ?? "ease";
  return {
    key,
    options,
    plain:
      plainStyleValue.test(animation) &&
      plainStyleValue.test(easing) &&
      !`${animation}${easing}`.includes("!"),
    timeline: createTimeline(options.maxBacklogMs),
    blocks: [],
  };
}

export function blockAnimation(animation: Animation, index: number): BlockAnimation {
  return (animation.blocks[index] ??= {
    committed: 0,
    last: 0,
    mark: null,
    plugin: null,
    streamdown: false,
    lists: new WeakMap(),
  });
}

/** The pass for one block, as the plugin starts its run: the clock, then the cursor at
 * this commit's first pass. */
export function beginPass(animation: Animation, block: BlockAnimation): AnimatePass {
  const { timeline, options } = animation;
  const at = timeline.now();
  if (block.mark === null) block.mark = timeline.mark();
  else timeline.rewind(block.mark);
  return {
    animation: options.animation ?? "fadeIn",
    duration: String(options.duration ?? 150),
    easing: options.easing ?? "ease",
    byChar: options.sep === "char",
    stagger: options.stagger ?? 40,
    shown: block.committed,
    cursor: timeline.mark(),
    now: at,
    backlog: options.maxBacklogMs ?? MAX_ANIMATION_BACKLOG_MS,
  };
}

/** After rill rendered the pass: the cursor moves on, and the block remembers its text. */
export function endPass(animation: Animation, block: BlockAnimation, pass: AnimatePass): void {
  animation.timeline.rewind(pass.next!);
  block.last = pass.chars!;
}

/** Streamdown's plugin for a block Streamdown's Block renders, seeded with what rill's
 * passes showed, and the rehype plugins with it last. */
export function streamdownPlugin(
  animation: Animation,
  block: BlockAnimation,
  rehypePlugins: Pluggable[]
): { plugin: AnimatePlugin; rehypePlugins: Pluggable[] } {
  if (!block.plugin) {
    const { maxBacklogMs: _, ...options } = animation.options;
    block.plugin = createAnimatePlugin({ ...options, timeline: animation.timeline } as never);
  }
  const plugin = block.plugin;
  plugin.setPrevContentLength(block.committed);
  let list = block.lists.get(rehypePlugins);
  if (!list) {
    list = [...rehypePlugins, plugin.rehypePlugin];
    block.lists.set(rehypePlugins, list);
  }
  return { plugin, rehypePlugins: list };
}

/** The plugin's `commit`, after the block's render is on screen. */
export function commitBlock(block: BlockAnimation, byStreamdown: boolean): void {
  block.committed = byStreamdown && block.plugin ? block.plugin.getLastRenderCharCount() : block.last;
  block.last = block.committed;
  block.mark = null;
}
