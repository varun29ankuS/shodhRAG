import { toast } from 'sonner';
import type { NotificationType } from '../hooks/useNotifications';

// Global reference to the notification center's add function — set by App on mount
let _addNotification: ((type: NotificationType, title: string, description?: string) => void) | null = null;

export function setNotificationHandler(handler: typeof _addNotification) {
  _addNotification = handler;
}

/** Show a toast AND push to the notification center */
export const notify = {
  success(title: string, opts?: { description?: string }) {
    toast.success(title, opts);
    _addNotification?.('success', title, opts?.description);
  },
  error(title: string, opts?: { description?: string }) {
    toast.error(title, opts);
    _addNotification?.('error', title, opts?.description);
  },
  info(title: string, opts?: { description?: string }) {
    toast.info(title, opts);
    _addNotification?.('info', title, opts?.description);
  },
  warning(title: string, opts?: { description?: string }) {
    toast.warning(title, opts);
    _addNotification?.('warning', title, opts?.description);
  },
};
