import { useState, useCallback, useRef } from 'react';

export type NotificationType = 'success' | 'error' | 'info' | 'warning';

export interface AppNotification {
  id: string;
  type: NotificationType;
  title: string;
  description?: string;
  timestamp: string;
  read: boolean;
}

const MAX_NOTIFICATIONS = 50;

export function useNotifications() {
  const [notifications, setNotifications] = useState<AppNotification[]>([]);
  const idCounter = useRef(0);

  const add = useCallback((type: NotificationType, title: string, description?: string) => {
    const id = `notif-${Date.now()}-${++idCounter.current}`;
    const notif: AppNotification = {
      id,
      type,
      title,
      description,
      timestamp: new Date().toISOString(),
      read: false,
    };
    setNotifications(prev => [notif, ...prev].slice(0, MAX_NOTIFICATIONS));
    return id;
  }, []);

  const markRead = useCallback((id: string) => {
    setNotifications(prev =>
      prev.map(n => n.id === id ? { ...n, read: true } : n)
    );
  }, []);

  const markAllRead = useCallback(() => {
    setNotifications(prev => prev.map(n => ({ ...n, read: true })));
  }, []);

  const remove = useCallback((id: string) => {
    setNotifications(prev => prev.filter(n => n.id !== id));
  }, []);

  const clearAll = useCallback(() => {
    setNotifications([]);
  }, []);

  const unreadCount = notifications.filter(n => !n.read).length;

  return {
    notifications,
    unreadCount,
    add,
    markRead,
    markAllRead,
    remove,
    clearAll,
  };
}
