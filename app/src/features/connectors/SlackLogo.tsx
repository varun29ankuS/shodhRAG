import React from 'react';
import { cn } from '../../lib/utils';

/*
 * The Slack mark (the multicolour hash), used only to identify the Slack connector.
 *
 * Source: the official artwork Slack serves on slack.com and its media kit
 * (https://a.slack-edge.com/9cc0056/marketing/img/nav/logo.svg, fetched 2026-10-26): its
 * eight paths and four colours (#E3066A, #00B3FF, #41B658, #FCC003) are copied unaltered.
 * Only the full-bleed clip path was dropped (it clips nothing, and its fixed id would
 * repeat when the mark shows more than once on a screen).
 *
 * Slack and the Slack logo are trademarks of Slack Technologies, LLC, a Salesforce
 * company. Shodh is not affiliated with or endorsed by Slack. Per Slack's brand
 * guidelines (slack.com/media-kit), the mark is shown unmodified, at its own proportions,
 * with clear space around it ({@link SlackMark} pads it), and only to say "this connects
 * to Slack".
 */

/** The bare mark, sized by `className`. Decorative: the name "Slack" is always next to it. */
export function SlackLogo({ className }: { className?: string }) {
  return (
    <svg viewBox="0 0 54 54" fill="none" xmlns="http://www.w3.org/2000/svg" aria-hidden="true" focusable="false" className={className}>
      <path d="M11.379 33.9993C11.379 37.1358 8.84512 39.6507 5.7276 39.6507C2.61008 39.6507 0.0572205 37.1168 0.0572205 33.9993C0.0572205 30.8817 2.5911 28.3479 5.70862 28.3479H11.36V33.9993H11.379Z" fill="#E3066A" />
      <path d="M14.1962 33.9997C14.1962 30.8632 16.7301 28.3483 19.8476 28.3483C22.9651 28.3483 25.499 30.8822 25.499 33.9997V48.1353C25.499 51.2718 22.9651 53.7867 19.8476 53.7867C16.7301 53.7867 14.1962 51.2718 14.1962 48.1353V33.9997Z" fill="#E3066A" />
      <path d="M19.8662 11.2673C16.7296 11.2673 14.2148 8.73347 14.2148 5.61594C14.2148 2.49842 16.7486 -0.0354538 19.8662 -0.0354538C22.9837 -0.0354538 25.5175 2.49842 25.5175 5.61594V11.2673H19.8662Z" fill="#00B3FF" />
      <path d="M19.8682 14.1334C23.0047 14.1334 25.5196 16.6673 25.5196 19.7848C25.5196 22.9023 22.9857 25.4362 19.8682 25.4362H5.67566C2.53916 25.4362 0.0242615 22.9023 0.0242615 19.7848C0.0242615 16.6673 2.55814 14.1334 5.67566 14.1334H19.8682Z" fill="#00B3FF" />
      <path d="M42.5323 19.7853C42.5323 16.6488 45.0662 14.1339 48.1837 14.1339C51.3012 14.1339 53.8351 16.6678 53.8351 19.7853C53.8351 22.9028 51.3012 25.4367 48.1837 25.4367H42.5323V19.7853Z" fill="#41B658" />
      <path d="M39.7126 19.7934C39.7126 22.9299 37.1787 25.4448 34.0612 25.4448C30.9436 25.4448 28.4098 22.911 28.4098 19.7934V5.61986C28.4098 2.48336 30.9436 -0.0315399 34.0612 -0.0315399C37.1787 -0.0315399 39.7126 2.48336 39.7126 5.61986V19.7934Z" fill="#41B658" />
      <path d="M34.0376 42.482C37.1741 42.482 39.689 45.0158 39.689 48.1334C39.689 51.2509 37.1552 53.7848 34.0376 53.7848C30.9201 53.7848 28.3862 51.2509 28.3862 48.1334V42.482H34.0376Z" fill="#FCC003" />
      <path d="M34.0381 39.6507C30.9016 39.6507 28.3867 37.1168 28.3867 33.9993C28.3867 30.8818 30.9206 28.3479 34.0381 28.3479H48.2306C51.3671 28.3479 53.882 30.8818 53.882 33.9993C53.882 37.1168 51.3482 39.6507 48.2306 39.6507H34.0381Z" fill="#FCC003" />
    </svg>
  );
}

/**
 * The mark on a white plate with clear space around it (a quarter of the plate on each
 * side), as an app icon: the same in light and dark themes, since the mark's colours are
 * made for a light ground.
 */
export function SlackMark({ size = 'md' }: { size?: 'sm' | 'md' | 'lg' }) {
  const plate = { sm: 'w-8 h-8 rounded-lg', md: 'w-10 h-10 rounded-[10px]', lg: 'w-12 h-12 rounded-xl' }[size];
  const mark = { sm: 'w-4 h-4', md: 'w-5 h-5', lg: 'w-6 h-6' }[size];
  return (
    <span aria-hidden="true" className={cn('shrink-0 inline-flex items-center justify-center bg-white border border-black/10 dark:border-white/10 shadow-sm', plate)}>
      <SlackLogo className={mark} />
    </span>
  );
}
