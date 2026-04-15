// Sleep helpers shared between keeper entrypoints.

/** Plain promise-based sleep. */
export function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

/**
 * Sleep that wakes early when the predicate flips. Used by keeper main
 * loops to react to SIGINT/SIGTERM within ~1s rather than after the full
 * poll interval.
 *
 * Internally checks the predicate every `tickMs` (default 1 second) — so
 * worst-case shutdown latency is one tick.
 */
export async function interruptibleSleep(
  totalMs: number,
  isStopped: () => boolean,
  tickMs: number = 1_000,
): Promise<void> {
  let remaining = totalMs;
  while (remaining > 0 && !isStopped()) {
    const chunk = Math.min(tickMs, remaining);
    await sleep(chunk);
    remaining -= chunk;
  }
}
