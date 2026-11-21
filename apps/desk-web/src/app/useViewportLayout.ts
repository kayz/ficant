import { useEffect, useState } from 'react';

export type ViewportLayoutMode = 'narrow' | 'wide';

export const WIDE_LAYOUT_MIN_WIDTH_PX = 768;

export function resolveViewportLayoutMode(widthPx: number): ViewportLayoutMode {
  return widthPx < WIDE_LAYOUT_MIN_WIDTH_PX ? 'narrow' : 'wide';
}

function readWindowViewportWidth(fallbackWidth: number): number {
  if (typeof window !== 'undefined' && typeof window.innerWidth === 'number' && window.innerWidth > 0) {
    return window.innerWidth;
  }
  return fallbackWidth;
}

export interface ViewportLayoutState {
  readonly width: number;
  readonly mode: ViewportLayoutMode;
}

export function useViewportLayout(initialViewportWidth?: number): ViewportLayoutState {
  const [width, setWidth] = useState<number>(() =>
    initialViewportWidth !== undefined
      ? initialViewportWidth
      : readWindowViewportWidth(1440),
  );

  useEffect(() => {
    if (initialViewportWidth !== undefined) {
      setWidth(initialViewportWidth);
    }
  }, [initialViewportWidth]);

  useEffect(() => {
    if (typeof window === 'undefined') {
      return;
    }

    const syncFromWindow = (): void => {
      if (typeof window.innerWidth === 'number' && window.innerWidth > 0) {
        setWidth(window.innerWidth);
      }
    };

    window.addEventListener('resize', syncFromWindow);

    let mediaQueryList: MediaQueryList | null = null;
    const handleMediaChange = (): void => {
      syncFromWindow();
    };

    if (typeof window.matchMedia === 'function') {
      try {
        mediaQueryList = window.matchMedia(`(min-width: ${WIDE_LAYOUT_MIN_WIDTH_PX}px)`);
        if (typeof mediaQueryList.addEventListener === 'function') {
          mediaQueryList.addEventListener('change', handleMediaChange);
        }
      } catch {
        mediaQueryList = null;
      }
    }

    return () => {
      window.removeEventListener('resize', syncFromWindow);
      if (mediaQueryList !== null && typeof mediaQueryList.removeEventListener === 'function') {
        mediaQueryList.removeEventListener('change', handleMediaChange);
      }
    };
  }, []);

  return {
    width,
    mode: resolveViewportLayoutMode(width),
  };
}
