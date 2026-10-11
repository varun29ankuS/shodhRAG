import { toast } from 'sonner';

/**
 * Toasts for the outcome of what the user just did. Lasting items (approvals,
 * suggestions, finished background work) are the Inbox's, from the backend.
 */
export const notify = {
  success(title: string, opts?: { description?: string }) {
    toast.success(title, opts);
  },
  error(title: string, opts?: { description?: string }) {
    toast.error(title, opts);
  },
  info(title: string, opts?: { description?: string }) {
    toast.info(title, opts);
  },
  warning(title: string, opts?: { description?: string }) {
    toast.warning(title, opts);
  },
};
