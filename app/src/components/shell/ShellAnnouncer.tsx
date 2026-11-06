import React, { useEffect, useState } from 'react';
import { subscribeAnnouncements } from '../../lib/announcer';

/**
 * The shell's one polite live region (lib/announcer), mounted once. Visually
 * hidden; screen readers hear each announcement after what they are reading.
 */
export function ShellAnnouncer() {
  const [message, setMessage] = useState('');
  useEffect(() => subscribeAnnouncements(text => {
    // The same words twice in a row are still announced: clear, then set.
    setMessage('');
    window.requestAnimationFrame(() => setMessage(text));
  }), []);
  return (
    <div role="status" aria-live="polite" aria-atomic="true" className="sr-only">
      {message}
    </div>
  );
}
