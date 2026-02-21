import React, { useState, useRef, useEffect } from 'react';
import { motion, AnimatePresence } from 'framer-motion';
import {
  Bell,
  CheckCircle,
  AlertCircle,
  Info,
  AlertTriangle,
  X,
  CheckCheck,
  Trash2,
} from 'lucide-react';
import { useTheme } from '../contexts/ThemeContext';
import { relativeTime } from '../utils/time';
import type { AppNotification } from '../hooks/useNotifications';

interface NotificationCenterProps {
  notifications: AppNotification[];
  unreadCount: number;
  onMarkRead: (id: string) => void;
  onMarkAllRead: () => void;
  onRemove: (id: string) => void;
  onClearAll: () => void;
}

const typeConfig = {
  success: { Icon: CheckCircle, label: 'Success' },
  error: { Icon: AlertCircle, label: 'Error' },
  info: { Icon: Info, label: 'Info' },
  warning: { Icon: AlertTriangle, label: 'Warning' },
};

export default function NotificationCenter({
  notifications,
  unreadCount,
  onMarkRead,
  onMarkAllRead,
  onRemove,
  onClearAll,
}: NotificationCenterProps) {
  const { colors } = useTheme();
  const [open, setOpen] = useState(false);
  const panelRef = useRef<HTMLDivElement>(null);

  // Close on outside click
  useEffect(() => {
    if (!open) return;
    const handler = (e: MouseEvent) => {
      if (panelRef.current && !panelRef.current.contains(e.target as Node)) {
        setOpen(false);
      }
    };
    document.addEventListener('mousedown', handler);
    return () => document.removeEventListener('mousedown', handler);
  }, [open]);

  const typeColor = (type: AppNotification['type']) => {
    switch (type) {
      case 'success': return colors.success;
      case 'error': return colors.error;
      case 'warning': return colors.warning;
      case 'info': return colors.secondary;
    }
  };

  return (
    <div className="relative" ref={panelRef}>
      {/* Bell button */}
      <button
        onClick={() => {
          setOpen(!open);
          if (!open && unreadCount > 0) {
            onMarkAllRead();
          }
        }}
        className="relative w-7 h-7 rounded-md flex items-center justify-center transition-colors"
        style={{ color: '#ef4444' }}
        title={unreadCount > 0 ? `${unreadCount} new notification${unreadCount > 1 ? 's' : ''}` : 'Notifications'}
      >
        <Bell className="w-4 h-4" />
        {unreadCount > 0 && (
          <motion.span
            initial={{ scale: 0 }}
            animate={{ scale: 1 }}
            className="absolute -top-0.5 -right-0.5 min-w-[14px] h-[14px] rounded-full flex items-center justify-center text-[9px] font-bold text-white px-0.5"
            style={{ backgroundColor: '#ef4444' }}
          >
            {unreadCount > 9 ? '9+' : unreadCount}
          </motion.span>
        )}
      </button>

      {/* Dropdown panel */}
      <AnimatePresence>
        {open && (
          <motion.div
            initial={{ opacity: 0, y: -8, scale: 0.96 }}
            animate={{ opacity: 1, y: 0, scale: 1 }}
            exit={{ opacity: 0, y: -8, scale: 0.96 }}
            transition={{ duration: 0.15 }}
            className="absolute right-0 top-9 z-50 w-80 rounded-lg border shadow-xl overflow-hidden"
            style={{ backgroundColor: colors.bgSecondary, borderColor: colors.border }}
          >
            {/* Header */}
            <div className="flex items-center justify-between px-3 py-2 border-b" style={{ borderColor: colors.border }}>
              <span className="text-xs font-semibold" style={{ color: colors.text }}>Notifications</span>
              <div className="flex items-center gap-1">
                {notifications.length > 0 && (
                  <>
                    <button
                      onClick={onMarkAllRead}
                      className="p-1 rounded transition-colors"
                      style={{ color: colors.textMuted }}
                      title="Mark all as read"
                    >
                      <CheckCheck className="w-3.5 h-3.5" />
                    </button>
                    <button
                      onClick={() => { onClearAll(); setOpen(false); }}
                      className="p-1 rounded transition-colors"
                      style={{ color: colors.textMuted }}
                      title="Clear all"
                    >
                      <Trash2 className="w-3.5 h-3.5" />
                    </button>
                  </>
                )}
              </div>
            </div>

            {/* Notification list */}
            <div className="max-h-80 overflow-y-auto">
              {notifications.length === 0 ? (
                <div className="py-8 text-center">
                  <Bell className="w-6 h-6 mx-auto mb-2" style={{ color: colors.textMuted }} />
                  <p className="text-xs" style={{ color: colors.textMuted }}>No notifications</p>
                </div>
              ) : (
                notifications.map(notif => {
                  const { Icon } = typeConfig[notif.type];
                  const clr = typeColor(notif.type);
                  return (
                    <div
                      key={notif.id}
                      className="flex items-start gap-2.5 px-3 py-2.5 transition-colors group cursor-default"
                      style={{
                        backgroundColor: notif.read ? 'transparent' : `${clr}06`,
                        borderLeft: notif.read ? '3px solid transparent' : `3px solid ${clr}`,
                      }}
                      onClick={() => onMarkRead(notif.id)}
                      onMouseEnter={e => { e.currentTarget.style.backgroundColor = colors.bgTertiary; }}
                      onMouseLeave={e => { e.currentTarget.style.backgroundColor = notif.read ? 'transparent' : `${clr}06`; }}
                    >
                      <div
                        className="w-6 h-6 rounded-full flex items-center justify-center shrink-0 mt-0.5"
                        style={{ backgroundColor: `${clr}14`, color: clr }}
                      >
                        <Icon className="w-3 h-3" />
                      </div>
                      <div className="flex-1 min-w-0">
                        <div className="flex items-start justify-between gap-2">
                          <p
                            className="text-[11px] font-medium leading-snug"
                            style={{ color: colors.text }}
                          >
                            {notif.title}
                          </p>
                          <button
                            onClick={(e) => { e.stopPropagation(); onRemove(notif.id); }}
                            className="opacity-0 group-hover:opacity-100 shrink-0 p-0.5 rounded transition-opacity"
                            style={{ color: colors.textMuted }}
                          >
                            <X className="w-3 h-3" />
                          </button>
                        </div>
                        {notif.description && (
                          <p className="text-[10px] mt-0.5 leading-snug truncate" style={{ color: colors.textMuted }}>
                            {notif.description}
                          </p>
                        )}
                        <p className="text-[9px] mt-1" style={{ color: colors.textTertiary }}>
                          {relativeTime(notif.timestamp)}
                        </p>
                      </div>
                    </div>
                  );
                })
              )}
            </div>
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  );
}
