/** Times in plain words, in the phone's time zone. */

/** "Good morning", "Good afternoon" or "Good evening". */
export function greeting(now: Date): string {
  const hour = now.getHours();
  if (hour < 5) return 'Good evening';
  if (hour < 12) return 'Good morning';
  if (hour < 17) return 'Good afternoon';
  return 'Good evening';
}

/** "Thursday 13 November". */
export function dayName(now: Date): string {
  return now.toLocaleDateString('en-GB', { weekday: 'long', day: 'numeric', month: 'long' });
}

function startOfDay(date: Date): number {
  return new Date(date.getFullYear(), date.getMonth(), date.getDate()).getTime();
}

/** "Just now", "5 minutes ago", "2 hours ago", "Yesterday", "Monday", "3 November". */
export function ago(iso: string, now: Date): string {
  const then = new Date(iso);
  if (Number.isNaN(then.getTime())) return '';
  const minutes = Math.floor((now.getTime() - then.getTime()) / 60_000);
  if (minutes < 1) return 'Just now';
  if (minutes < 60) return minutes === 1 ? '1 minute ago' : `${minutes} minutes ago`;
  const days = Math.round((startOfDay(now) - startOfDay(then)) / 86_400_000);
  if (days <= 0) {
    const hours = Math.floor(minutes / 60);
    return hours === 1 ? '1 hour ago' : `${hours} hours ago`;
  }
  if (days === 1) return 'Yesterday';
  if (days < 7) return then.toLocaleDateString('en-GB', { weekday: 'long' });
  return then.toLocaleDateString('en-GB', { day: 'numeric', month: 'long' });
}

export type Period = 'Today' | 'Yesterday' | 'This week' | 'Earlier';

/** Which group of a list a time falls in. */
export function period(iso: string, now: Date): Period {
  const then = new Date(iso);
  if (Number.isNaN(then.getTime())) return 'Earlier';
  const days = Math.round((startOfDay(now) - startOfDay(then)) / 86_400_000);
  if (days <= 0) return 'Today';
  if (days === 1) return 'Yesterday';
  if (days < 7) return 'This week';
  return 'Earlier';
}
