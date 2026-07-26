import { useEffect, useState } from "react";

const REDUCED_MOTION_QUERY = "(prefers-reduced-motion: reduce)";

export function prefersReducedMotion(): boolean {
  return typeof window !== "undefined" && window.matchMedia(REDUCED_MOTION_QUERY).matches;
}

/** Types `text` in one character at a time; jumps straight to the full string under reduced motion. */
export function useTypedText(text: string, msPerChar = 32, startDelayMs = 300) {
  const [chars, setChars] = useState(0);
  const [done, setDone] = useState(false);

  useEffect(() => {
    if (prefersReducedMotion()) {
      setChars(text.length);
      setDone(true);
      return;
    }

    setChars(0);
    setDone(false);

    let cancelled = false;
    const timers: ReturnType<typeof setTimeout>[] = [];

    function tick(i: number) {
      if (cancelled) return;
      setChars(i);
      if (i >= text.length) {
        setDone(true);
        return;
      }
      timers.push(setTimeout(() => tick(i + 1), msPerChar));
    }

    timers.push(setTimeout(() => tick(0), startDelayMs));
    return () => {
      cancelled = true;
      timers.forEach(clearTimeout);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  return { text: text.slice(0, chars), done };
}

/** Reveals `count` items one at a time on an interval, starting once `active` is true. */
export function useStaggerReveal(count: number, active: boolean, stepMs = 90) {
  const [visible, setVisible] = useState(0);

  useEffect(() => {
    if (!active) return;

    if (prefersReducedMotion()) {
      setVisible(count);
      return;
    }

    let cancelled = false;
    const timers: ReturnType<typeof setTimeout>[] = [];
    for (let i = 1; i <= count; i++) {
      timers.push(
        setTimeout(() => {
          if (!cancelled) setVisible(i);
        }, i * stepMs),
      );
    }
    return () => {
      cancelled = true;
      timers.forEach(clearTimeout);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [active]);

  return visible;
}
