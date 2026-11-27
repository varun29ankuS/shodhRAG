/**
 * The page frame's measures: one left edge, one width and one title position for every view
 * but Ask (UI structure audit 2026-11-23, rule 1), and the corner kept clear for the floating
 * chat's pill (rule 4). `PageFrame.tsx` draws them; `app/tests/pageFrame.test.ts` checks
 * that every view uses the frame and that the classes below say these numbers.
 *
 * No imports: the Node tests load it.
 */

/** The content's widest, in CSS pixels (Tailwind `max-w-5xl`). */
export const PAGE_MAX_WIDTH_PX = 1024;

/** The space at each side of the content (Tailwind `px-8`). */
export const PAGE_SIDE_PX = 32;

/** From the top of the view to the title row (Tailwind `pt-7`). */
export const PAGE_TOP_PX = 28;

/** The title row's height: the title, and the primary action's slot at its right (`min-h-9`). */
export const PAGE_TITLE_ROW_PX = 36;

/**
 * The floating chat's pill is 40px tall and sits 16px above the window's bottom edge
 * (`dockLayout`): every frame ends with this much empty space, so the pill never covers a
 * control at the end of a page.
 */
export const DOCK_GUTTER_PX = 56;

/** The end of a page: 28px, plus the gutter (the `page-end` spacing, `pb-page-end`). */
export const PAGE_BOTTOM_PX = 28 + DOCK_GUTTER_PX;

/** The class that ends a page or a pane with `PAGE_BOTTOM_PX`. */
export const PAGE_END_CLASS = 'pb-page-end';

/** The frame's scroller: the whole view scrolls as one page. */
export const PAGE_SCROLLER_CLASS = 'h-full overflow-y-auto scrollbar-thin bg-shodh-ground text-shodh-text';

/** A frame whose content fills the view and scrolls in its own panes (a calendar, a split view). */
export const PAGE_FILL_CLASS = 'h-full flex flex-col overflow-hidden bg-shodh-ground text-shodh-text';

/** The column every frame shares: the left edge, the top and the gutter. */
export const PAGE_COLUMN_CLASS = 'w-full px-8 pt-7 pb-page-end flex flex-col gap-6';

/** A frame of the page's width, centred. */
export const PAGE_WIDTH_CLASS = 'mx-auto max-w-5xl';

/** The title row: the title at the left, the primary action's slot at the right. */
export const PAGE_TITLE_ROW_CLASS = 'flex flex-wrap items-center gap-x-4 gap-y-2 min-h-9';

/** The page title: one size and weight on every view. */
export const PAGE_TITLE_CLASS = 'm-0 shrink-0 whitespace-nowrap text-title font-semibold text-shodh-text';

/** The reserved slot for a view's one primary action. It keeps its place when empty. */
export const PAGE_ACTION_SLOT_CLASS = 'ms-auto min-w-0 flex flex-wrap items-center justify-end gap-2 min-h-9';
