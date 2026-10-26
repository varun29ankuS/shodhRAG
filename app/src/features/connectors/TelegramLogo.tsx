import React, { useId } from 'react';
import { cn } from '../../lib/utils';

/*
 * The Telegram logo (the paper plane on the blue circle), used only to identify the
 * Telegram connector.
 *
 * Source: the official artwork Telegram publishes on its press page
 * (https://telegram.org/tour/screenshots, "Download Telegram Logos", Logo.svg, fetched
 * 2026-10-27): its circle, its gradient (#2AABEE to #229ED9) and its plane path are copied
 * unaltered. Only the gradient's id is made unique per use (a fixed id would repeat when
 * the logo shows more than once on a screen) and the editor's title and comment are
 * dropped.
 *
 * Telegram and the Telegram logo are trademarks of Telegram FZ-LLC. Shodh is not
 * affiliated with or endorsed by Telegram. Telegram's press page allows its logos for
 * "forward to Telegram" buttons and the like as long as people understand the user is not
 * representing Telegram officially; here the logo only says "this connects to Telegram".
 */

/** The bare logo, sized by `className`. Decorative: the name "Telegram" is always next to it. */
export function TelegramLogo({ className }: { className?: string }) {
  const gradient = `tg-${useId().replace(/[^A-Za-z0-9_-]/g, '')}`;
  return (
    <svg viewBox="0 0 1000 1000" xmlns="http://www.w3.org/2000/svg" aria-hidden="true" focusable="false" className={className}>
      <defs>
        <linearGradient x1="50%" y1="0%" x2="50%" y2="99.2583404%" id={gradient}>
          <stop stopColor="#2AABEE" offset="0%" />
          <stop stopColor="#229ED9" offset="100%" />
        </linearGradient>
      </defs>
      <g stroke="none" strokeWidth="1" fill="none" fillRule="evenodd">
        <circle fill={`url(#${gradient})`} cx="500" cy="500" r="500" />
        <path
          d="M226.328419,494.722069 C372.088573,431.216685 469.284839,389.350049 517.917216,369.122161 C656.772535,311.36743 685.625481,301.334815 704.431427,301.003532 C708.567621,300.93067 717.815839,301.955743 723.806446,306.816707 C728.864797,310.92121 730.256552,316.46581 730.922551,320.357329 C731.588551,324.248848 732.417879,333.113828 731.758626,340.040666 C724.234007,419.102486 691.675104,610.964674 675.110982,699.515267 C668.10208,736.984342 654.301336,749.547532 640.940618,750.777006 C611.904684,753.448938 589.856115,731.588035 561.733393,713.153237 C517.726886,684.306416 492.866009,666.349181 450.150074,638.200013 C400.78442,605.66878 432.786119,587.789048 460.919462,558.568563 C468.282091,550.921423 596.21508,434.556479 598.691227,424.000355 C599.00091,422.680135 599.288312,417.758981 596.36474,415.160431 C593.441168,412.561881 589.126229,413.450484 586.012448,414.157198 C581.598758,415.158943 511.297793,461.625274 375.109553,553.556189 C355.154858,567.258623 337.080515,573.934908 320.886524,573.585046 C303.033948,573.199351 268.692754,563.490928 243.163606,555.192408 C211.851067,545.013936 186.964484,539.632504 189.131547,522.346309 C190.260287,513.342589 202.659244,504.134509 226.328419,494.722069 Z"
          fill="#FFFFFF"
        />
      </g>
    </svg>
  );
}

/**
 * The logo with clear space around it on a plate, matching {@link SlackMark}'s sizes, so
 * the gallery's tiles line up. The circle is the logo's own ground; the plate only pads it.
 */
export function TelegramMark({ size = 'md' }: { size?: 'sm' | 'md' | 'lg' }) {
  const plate = { sm: 'w-8 h-8 rounded-lg', md: 'w-10 h-10 rounded-[10px]', lg: 'w-12 h-12 rounded-xl' }[size];
  const mark = { sm: 'w-5 h-5', md: 'w-6 h-6', lg: 'w-7 h-7' }[size];
  return (
    <span aria-hidden="true" className={cn('shrink-0 inline-flex items-center justify-center bg-white border border-black/10 dark:border-white/10 shadow-sm', plate)}>
      <TelegramLogo className={mark} />
    </span>
  );
}
