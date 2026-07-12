import { useCallback, useEffect, useRef, useState } from "react";

const ANNOUNCEMENT_INTERVAL_MS = 250;

export function usePoliteAnnouncements(initial: string): readonly [string, (message: string) => void] {
  const [current, setCurrent] = useState(initial);
  const currentRef = useRef(initial);
  const queueRef = useRef<string[]>([]);
  const timerRef = useRef<number | undefined>(undefined);

  const flush = useCallback(() => {
    timerRef.current = undefined;
    const next = queueRef.current.shift();
    if (next !== undefined && next !== currentRef.current) {
      currentRef.current = next;
      setCurrent(next);
    }
    if (queueRef.current.length > 0) {
      timerRef.current = window.setTimeout(flush, ANNOUNCEMENT_INTERVAL_MS);
    }
  }, []);

  const announce = useCallback((message: string) => {
    const lastQueued = queueRef.current.at(-1) ?? currentRef.current;
    if (!message || message === lastQueued) return;
    queueRef.current.push(message);
    if (timerRef.current === undefined) {
      timerRef.current = window.setTimeout(flush, ANNOUNCEMENT_INTERVAL_MS);
    }
  }, [flush]);

  useEffect(() => () => {
    if (timerRef.current !== undefined) window.clearTimeout(timerRef.current);
    queueRef.current = [];
  }, []);

  return [current, announce] as const;
}
