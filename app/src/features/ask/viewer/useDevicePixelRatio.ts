import { useEffect, useState } from 'react';
import { currentPixelRatio, watchPixelRatio } from './pixelRatio';

/** `window.devicePixelRatio`, updated when the window moves to a monitor with another scale. */
export function useDevicePixelRatio(): number {
  const [ratio, setRatio] = useState(() => currentPixelRatio(window.devicePixelRatio));
  useEffect(() => {
    // The ratio may have changed between the first render and this effect.
    setRatio(currentPixelRatio(window.devicePixelRatio));
    return watchPixelRatio(window, setRatio);
  }, []);
  return ratio;
}
