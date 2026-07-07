// Dependency-free windowing for the Library screen. A library can hold thousands
// of titles (a real Movies library is easily 3–4k), and rendering every poster
// card / table row at once means thousands of DOM nodes and poster <img> tags on
// the first paint — enough to make scrolling stutter and the tab hitch on load.
//
// Rather than pull in a virtual-list dependency (the UI is SRCL-only), we reveal
// the already-fetched list incrementally: render an initial slice, then grow the
// slice by a step each time a sentinel element near the bottom scrolls into view.
// Only the revealed rows are in the DOM, so the node/poster count stays bounded
// while the user scrolls the full list. This is pure glue — no UI primitives —
// so it stays clear of the SRCL-only rule.
//
// IntersectionObserver isn't available in every environment (notably jsdom under
// the test runner). When it's absent we reveal everything up front, so the list
// still renders in full — the incremental behaviour is a progressive enhancement
// on top of a complete render, never a gate on it.

import * as React from 'react';

/** How many items to show before the user scrolls, and how many to add per step. */
export const REVEAL_INITIAL = 60;
// Add a smallish slice per reveal so mounting a batch is a sub-frame amount of
// work — a large batch (60+) mounting synchronously drops a frame and the scroll
// hitches. Smaller, more frequent reveals (each well ahead of the viewport via
// the observer's rootMargin below) keep the scroll smooth.
export const REVEAL_STEP = 30;

/**
 * How far below the viewport the sentinel triggers the next reveal. Scaled to the
 * viewport (~1.5 screens ahead) rather than a fixed px so a fast scroll on a tall
 * display can't outrun it: the next batch mounts and its posters start loading
 * well before the user reaches them, off the moment they're looking at.
 */
function prefetchRootMargin(): string {
  const vh = typeof window !== 'undefined' ? window.innerHeight : 800;
  return `${Math.round(vh * 1.5)}px 0px`;
}

export interface IncrementalReveal {
  /** How many of the list's items to render right now. */
  count: number;
  /** Whether more items remain beyond the current slice. */
  hasMore: boolean;
  /** Attach to a sentinel element rendered just after the last visible item. */
  sentinelRef: (node: Element | null) => void;
}

function supportsObserver(): boolean {
  return typeof IntersectionObserver !== 'undefined';
}

/**
 * Grow a render window over a list of `total` items as a sentinel scrolls into
 * view. `resetKey` changes (e.g. the active library, filter, or sort) snap the
 * window back to the initial slice so a new list starts from the top.
 */
export function useIncrementalReveal(total: number, resetKey: string): IncrementalReveal {
  // When IntersectionObserver is unavailable we can't observe scroll, so reveal
  // the whole list immediately — a complete (if un-windowed) render beats a list
  // stuck at its first slice with no way to grow.
  const observable = supportsObserver();
  const initial = observable ? Math.min(REVEAL_INITIAL, total) : total;

  const [count, setCount] = React.useState(initial);
  const observerRef = React.useRef<IntersectionObserver | null>(null);
  const sentinelNodeRef = React.useRef<Element | null>(null);

  // A new list (library switch, filter/sort change) starts from the top. Clamp to
  // the fresh total so a shorter list never shows a stale, larger count.
  React.useEffect(() => {
    setCount(observable ? Math.min(REVEAL_INITIAL, total) : total);
  }, [resetKey, total, observable]);

  // Grow the window whenever the sentinel is visible. Keeping the callback in a
  // ref-stable observer lets us re-observe the same node across renders without
  // tearing down the observer each time the count changes.
  const grow = React.useCallback(() => {
    setCount((prev) => (prev < total ? Math.min(prev + REVEAL_STEP, total) : prev));
  }, [total]);

  const sentinelRef = React.useCallback(
    (node: Element | null) => {
      sentinelNodeRef.current = node;
      if (!observable) return;
      if (observerRef.current) {
        observerRef.current.disconnect();
        observerRef.current = null;
      }
      if (!node) return;
      const observer = new IntersectionObserver(
        (entries) => {
          for (const entry of entries) {
            if (entry.isIntersecting) grow();
          }
        },
        // Reveal the next slice well before the sentinel is on screen (scaled to
        // the viewport), so the batch mounts and its posters load ahead of the
        // user rather than at the moment they scroll to the bottom.
        { rootMargin: prefetchRootMargin() }
      );
      observer.observe(node);
      observerRef.current = observer;
    },
    [observable, grow]
  );

  // If the count jumped (list grew) while the sentinel is still on screen, a
  // single intersection callback only adds one step. Re-observing after each
  // count change lets a fast scroll or a short viewport fill in multiple steps.
  React.useEffect(() => {
    const node = sentinelNodeRef.current;
    const observer = observerRef.current;
    if (observable && node && observer) {
      observer.unobserve(node);
      observer.observe(node);
    }
  }, [count, observable]);

  React.useEffect(() => {
    return () => {
      if (observerRef.current) observerRef.current.disconnect();
    };
  }, []);

  return { count, hasMore: count < total, sentinelRef };
}
