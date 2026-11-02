import React from 'react';
import { cn } from '../../lib/utils';

/*
 * The GitHub mark (the Invertocat), used only to say "this connects to GitHub".
 *
 * Source: Primer Octicons `mark-github-16` (@primer/octicons 19.8.0,
 * build/svg/mark-github-16.svg, MIT licence, fetched 2026-11-02): its path is copied
 * unaltered. It is drawn in black on a white plate, GitHub's highest-contrast colourway,
 * in light and dark themes alike, never recoloured, and always next to the word
 * "GitHub" and below Shodh's own name and mark in prominence.
 *
 * GitHub and the Invertocat are trademarks of GitHub, Inc., registered in the United
 * States and other countries. Shodh is not affiliated with or endorsed by GitHub.
 * GitHub's logo guidelines (brand.github.com/foundations/logo) allow a permitted logo
 * "to inform others that your project integrates with GitHub".
 */

const MARK =
  'M8 0c4.42 0 8 3.58 8 8a8.013 8.013 0 0 1-5.45 7.59c-.4.08-.55-.17-.55-.38 0-.27.01-1.13.01-2.2 0-.75-.25-1.23-.54-1.48 1.78-.2 3.65-.88 3.65-3.95 0-.88-.31-1.59-.82-2.15.08-.2.36-1.02-.08-2.12 0 0-.67-.22-2.2.82-.64-.18-1.32-.27-2-.27-.68 0-1.36.09-2 .27-1.53-1.03-2.2-.82-2.2-.82-.44 1.1-.16 1.92-.08 2.12-.51.56-.82 1.28-.82 2.15 0 3.06 1.86 3.75 3.64 3.95-.23.2-.44.55-.51 1.07-.46.21-1.61.55-2.33-.66-.15-.24-.6-.83-1.23-.82-.67.01-.27.38.01.53.34.19.73.9.82 1.13.16.45.68 1.31 2.69.94 0 .67.01 1.3.01 1.49 0 .21-.15.45-.55.38A7.995 7.995 0 0 1 0 8c0-4.42 3.58-8 8-8Z';

/** The bare mark, sized by `className`. Decorative: the name "GitHub" is always next to it. */
export function GitHubLogo({ className }: { className?: string }) {
  return (
    <svg viewBox="0 0 16 16" xmlns="http://www.w3.org/2000/svg" aria-hidden="true" focusable="false" className={className}>
      <path d={MARK} fill="#000000" />
    </svg>
  );
}

/** The mark on a white plate, matching the other connectors' sizes, so the tiles line up. */
export function GitHubMark({ size = 'md' }: { size?: 'sm' | 'md' | 'lg' }) {
  const plate = { sm: 'w-8 h-8 rounded-lg', md: 'w-10 h-10 rounded-[10px]', lg: 'w-12 h-12 rounded-xl' }[size];
  const mark = { sm: 'w-5 h-5', md: 'w-6 h-6', lg: 'w-7 h-7' }[size];
  return (
    <span aria-hidden="true" className={cn('shrink-0 inline-flex items-center justify-center bg-white border border-black/10 dark:border-white/10 shadow-sm', plate)}>
      <GitHubLogo className={mark} />
    </span>
  );
}
