import { forwardRef, useId } from 'react';
import type React from 'react';
import { ChevronLeft } from 'lucide-react';
import { cn } from '../../lib/utils';
import {
  PAGE_ACTION_SLOT_CLASS,
  PAGE_COLUMN_CLASS,
  PAGE_FILL_CLASS,
  PAGE_SCROLLER_CLASS,
  PAGE_TITLE_CLASS,
  PAGE_TITLE_ROW_CLASS,
  PAGE_WIDTH_CLASS,
} from './frameMeasures';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-shodh-ground';

interface PageFrameProps extends Omit<React.HTMLAttributes<HTMLDivElement>, 'title'> {
  /** The page's title: its `h1`, at the same place on every view. */
  title: string;
  /** One line under the title, when the page needs saying what it is. */
  lede?: React.ReactNode;
  /** The view's one primary action ("Add folder"), in the slot at the title row's right. */
  action?: React.ReactNode;
  /** A row under the title for view switches, filters and search. */
  toolbar?: React.ReactNode;
  /** A page opened from another: where Back goes. */
  back?: { label: string; onClick: () => void };
  /** `page`: the shared width, centred. `full`: the whole view (a split view, a calendar). */
  width?: 'page' | 'full';
  /**
   * The content fills the rest of the view and scrolls in its own panes, instead of the
   * whole page scrolling. Such content keeps the pill's corner clear itself.
   */
  fill?: boolean;
  /**
   * False for a page opened inside the view that draws its own heading (a workspace, a
   * connector, a paper): the frame keeps the edge, the width and the gutter, and `title`
   * names the region.
   */
  heading?: boolean;
  children: React.ReactNode;
}

/**
 * The frame of every view but Ask: one left edge, one width, the title at one top, a reserved
 * slot for the primary action at the title row's right, an optional toolbar row, and a
 * gutter at the end for the floating chat's pill. Moving between views moves nothing but
 * the content.
 */
export const PageFrame = forwardRef<HTMLDivElement, PageFrameProps>(function PageFrame(
  { title, lede, action, toolbar, back, width = 'page', fill = false, heading = true, className, children, ...rest },
  ref,
) {
  const titleId = useId();
  const labelled = heading ? { 'aria-labelledby': titleId } : { 'aria-label': title };
  const head = heading && (
    <>
      {back && (
        <button
          type="button"
          onClick={back.onClick}
          className={cn('self-start -mb-3 inline-flex items-center gap-1 h-7 px-1.5 -ms-1.5 rounded-md text-small text-shodh-text-secondary hover:bg-shodh-raised hover:text-shodh-text transition-colors duration-micro', FOCUS_RING)}
        >
          <ChevronLeft className="w-4 h-4 rtl:rotate-180" aria-hidden="true" />
          {back.label}
        </button>
      )}
      <header className="flex flex-col gap-1.5" data-page-header="">
        <div className={PAGE_TITLE_ROW_CLASS}>
          <h1 id={titleId} className={PAGE_TITLE_CLASS} data-page-title="">
            {title}
          </h1>
          <div className={PAGE_ACTION_SLOT_CLASS} data-page-action="">
            {action}
          </div>
        </div>
        {lede && <p className="m-0 max-w-[640px] text-small text-shodh-text-secondary">{lede}</p>}
      </header>
      {toolbar && (
        <div className="flex flex-wrap items-center gap-3" data-page-toolbar="">
          {toolbar}
        </div>
      )}
    </>
  );
  if (fill) {
    return (
      <div ref={ref} {...rest} {...labelled} data-page-frame="fill" className={cn(PAGE_FILL_CLASS, className)}>
        {head && <div className={cn('shrink-0 w-full px-8 pt-7 pb-4 flex flex-col gap-4', width === 'page' && PAGE_WIDTH_CLASS)}>{head}</div>}
        <div className="flex-1 min-h-0 flex flex-col" data-page-body="">
          {children}
        </div>
      </div>
    );
  }
  return (
    <div ref={ref} {...rest} {...labelled} data-page-frame="page" className={cn(PAGE_SCROLLER_CLASS, className)}>
      <div className={cn(PAGE_COLUMN_CLASS, width === 'page' && PAGE_WIDTH_CLASS)}>
        {head}
        {children}
      </div>
    </div>
  );
});

export default PageFrame;
