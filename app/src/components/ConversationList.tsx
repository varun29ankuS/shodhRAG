import React, { useState, useRef, useEffect } from 'react';
import { motion, AnimatePresence, Reorder } from 'framer-motion';
import {
  Plus,
  MessageSquare,
  MoreHorizontal,
  Pencil,
  Trash2,
  Pin,
  PinOff,
  Check,
  X,
  GripVertical,
} from 'lucide-react';
import { useTheme } from '../contexts/ThemeContext';
import { sourceColor } from '../utils/colors';
import { relativeTime } from '../utils/time';
import type { Conversation } from '../hooks/useConversations';

interface ConversationListProps {
  conversations: Conversation[];
  activeConversationId: string | null;
  collapsed: boolean;
  onSelect: (id: string) => void;
  onNew: () => void;
  onDelete: (id: string) => void;
  onRename: (id: string, title: string) => void;
  onPin: (id: string) => void;
  onReorder?: (reordered: Conversation[]) => void;
}


export default function ConversationList({
  conversations,
  activeConversationId,
  collapsed,
  onSelect,
  onNew,
  onDelete,
  onRename,
  onPin,
  onReorder,
}: ConversationListProps) {
  const { colors } = useTheme();
  const [contextMenuId, setContextMenuId] = useState<string | null>(null);
  const [editingId, setEditingId] = useState<string | null>(null);
  const [editValue, setEditValue] = useState('');
  const editInputRef = useRef<HTMLInputElement>(null);
  const contextRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (editingId && editInputRef.current) {
      editInputRef.current.focus();
      editInputRef.current.select();
    }
  }, [editingId]);

  useEffect(() => {
    const handleClickOutside = (e: MouseEvent) => {
      if (contextRef.current && !contextRef.current.contains(e.target as Node)) {
        setContextMenuId(null);
      }
    };
    if (contextMenuId) {
      document.addEventListener('mousedown', handleClickOutside);
      return () => document.removeEventListener('mousedown', handleClickOutside);
    }
  }, [contextMenuId]);

  const startRename = (id: string, currentTitle: string) => {
    setEditingId(id);
    setEditValue(currentTitle);
    setContextMenuId(null);
  };

  const commitRename = () => {
    if (editingId && editValue.trim()) {
      onRename(editingId, editValue.trim());
    }
    setEditingId(null);
  };

  const cancelRename = () => {
    setEditingId(null);
  };

  const pinned = conversations.filter(c => c.pinned);
  const unpinned = conversations.filter(c => !c.pinned);

  if (collapsed) {
    return (
      <div className="flex flex-col items-center gap-1 py-2">
        <button
          onClick={onNew}
          className="w-8 h-8 rounded-lg flex items-center justify-center transition-colors"
          style={{ color: colors.textSecondary }}
          title="New Chat"
        >
          <Plus className="w-4 h-4" />
        </button>
        {conversations.slice(0, 5).map(conv => {
          const clr = conv.spaceName ? sourceColor(conv.spaceName) : colors.primary;
          const isActive = conv.id === activeConversationId;
          return (
            <button
              key={conv.id}
              onClick={() => onSelect(conv.id)}
              className="w-8 h-8 rounded-lg flex items-center justify-center transition-colors"
              style={{
                backgroundColor: isActive ? `${clr}14` : 'transparent',
                color: isActive ? clr : colors.textTertiary,
              }}
              title={conv.title}
            >
              <MessageSquare className="w-3.5 h-3.5" />
            </button>
          );
        })}
      </div>
    );
  }

  const renderItemContent = (conv: Conversation, draggable?: boolean) => {
    const isActive = conv.id === activeConversationId;
    const isEditing = editingId === conv.id;
    const accentColor = conv.spaceName ? sourceColor(conv.spaceName) : colors.primary;

    return (
      <div className="relative group">
        <button
          onClick={() => !isEditing && onSelect(conv.id)}
          className="w-full text-left px-3 py-2 rounded-lg flex items-center gap-2.5 transition-all duration-150"
          style={{
            backgroundColor: isActive ? `${accentColor}14` : 'transparent',
            borderLeft: isActive ? `2px solid ${accentColor}` : '2px solid transparent',
          }}
        >
          {draggable && (
            <div
              className="shrink-0 cursor-grab active:cursor-grabbing opacity-0 group-hover:opacity-60 transition-opacity"
              onPointerDown={e => e.stopPropagation()}
              style={{ color: colors.textMuted, touchAction: 'none' }}
            >
              <GripVertical className="w-3 h-3" />
            </div>
          )}
          <MessageSquare
            className="w-3.5 h-3.5 shrink-0"
            style={{ color: isActive ? accentColor : colors.textMuted }}
          />
          <div className="flex-1 min-w-0">
            {isEditing ? (
              <div className="flex items-center gap-1">
                <input
                  ref={editInputRef}
                  type="text"
                  value={editValue}
                  onChange={e => setEditValue(e.target.value)}
                  onKeyDown={e => {
                    if (e.key === 'Enter') commitRename();
                    if (e.key === 'Escape') cancelRename();
                  }}
                  onBlur={commitRename}
                  className="w-full text-xs bg-transparent border-b outline-none py-0.5"
                  style={{ color: colors.text, borderColor: colors.primary }}
                />
                <button onClick={commitRename} className="shrink-0">
                  <Check className="w-3 h-3" style={{ color: colors.success }} />
                </button>
                <button onClick={cancelRename} className="shrink-0">
                  <X className="w-3 h-3" style={{ color: colors.textMuted }} />
                </button>
              </div>
            ) : (
              <>
                <div
                  className="text-xs font-medium truncate leading-tight"
                  style={{ color: isActive ? colors.text : colors.textSecondary }}
                >
                  {conv.title}
                </div>
                <div className="flex items-center gap-1.5 mt-0.5 flex-wrap">
                  {conv.spaceName && (
                    <span
                      className="text-[9px] font-medium px-1.5 py-px rounded-full truncate max-w-[80px]"
                      style={{
                        backgroundColor: `${sourceColor(conv.spaceName)}18`,
                        color: sourceColor(conv.spaceName),
                        border: `1px solid ${sourceColor(conv.spaceName)}30`,
                      }}
                      title={conv.spaceName}
                    >
                      {conv.spaceName}
                    </span>
                  )}
                  <span className="text-[10px]" style={{ color: colors.textMuted }}>
                    {conv.messages.length} msgs
                  </span>
                  <span className="text-[10px]" style={{ color: colors.textMuted }}>
                    {relativeTime(conv.updatedAt)}
                  </span>
                </div>
              </>
            )}
          </div>
          {!isEditing && (
            <div className="opacity-0 group-hover:opacity-100 transition-opacity shrink-0">
              <button
                onClick={e => {
                  e.stopPropagation();
                  setContextMenuId(contextMenuId === conv.id ? null : conv.id);
                }}
                className="w-5 h-5 rounded flex items-center justify-center transition-colors"
                style={{ color: colors.textMuted }}
              >
                <MoreHorizontal className="w-3.5 h-3.5" />
              </button>
            </div>
          )}
        </button>

        <AnimatePresence>
          {contextMenuId === conv.id && (
            <motion.div
              ref={contextRef}
              initial={{ opacity: 0, scale: 0.95, y: -4 }}
              animate={{ opacity: 1, scale: 1, y: 0 }}
              exit={{ opacity: 0, scale: 0.95, y: -4 }}
              transition={{ duration: 0.1 }}
              className="absolute right-2 top-full z-50 rounded-lg border py-1 min-w-[140px]"
              style={{
                backgroundColor: colors.bgSecondary,
                borderColor: colors.border,
                boxShadow: '0 4px 12px rgba(0,0,0,0.15)',
              }}
            >
              <button
                onClick={() => startRename(conv.id, conv.title)}
                className="w-full text-left px-3 py-1.5 text-xs flex items-center gap-2 transition-colors"
                style={{ color: colors.textSecondary }}
                onMouseEnter={e => (e.currentTarget.style.backgroundColor = colors.bgHover)}
                onMouseLeave={e => (e.currentTarget.style.backgroundColor = 'transparent')}
              >
                <Pencil className="w-3 h-3" /> Rename
              </button>
              <button
                onClick={() => {
                  onPin(conv.id);
                  setContextMenuId(null);
                }}
                className="w-full text-left px-3 py-1.5 text-xs flex items-center gap-2 transition-colors"
                style={{ color: colors.textSecondary }}
                onMouseEnter={e => (e.currentTarget.style.backgroundColor = colors.bgHover)}
                onMouseLeave={e => (e.currentTarget.style.backgroundColor = 'transparent')}
              >
                {conv.pinned ? <PinOff className="w-3 h-3" /> : <Pin className="w-3 h-3" />}
                {conv.pinned ? 'Unpin' : 'Pin'}
              </button>
              <div className="my-1 border-t" style={{ borderColor: colors.border }} />
              <button
                onClick={() => {
                  onDelete(conv.id);
                  setContextMenuId(null);
                }}
                className="w-full text-left px-3 py-1.5 text-xs flex items-center gap-2 transition-colors"
                style={{ color: colors.error }}
                onMouseEnter={e => (e.currentTarget.style.backgroundColor = colors.bgHover)}
                onMouseLeave={e => (e.currentTarget.style.backgroundColor = 'transparent')}
              >
                <Trash2 className="w-3 h-3" /> Delete
              </button>
            </motion.div>
          )}
        </AnimatePresence>
      </div>
    );
  };

  const handleReorder = (newUnpinned: Conversation[]) => {
    if (!onReorder) return;
    onReorder([...pinned, ...newUnpinned]);
  };

  return (
    <div className="flex flex-col">
      <div className="flex items-center justify-between px-3 mb-2">
        <span className="text-[10px] font-bold tracking-widest" style={{ color: colors.textMuted }}>
          THREADS
        </span>
        <button
          onClick={onNew}
          className="w-5 h-5 rounded flex items-center justify-center transition-colors"
          style={{ color: colors.textTertiary }}
          title="New Chat"
        >
          <Plus className="w-3.5 h-3.5" />
        </button>
      </div>

      <div className="space-y-0.5 px-1">
        {pinned.length > 0 && (
          <>
            <div className="px-2 py-1">
              <span className="text-[9px] font-semibold tracking-wider" style={{ color: colors.textMuted }}>
                PINNED
              </span>
            </div>
            <AnimatePresence mode="popLayout">
              {pinned.map(conv => (
                <motion.div
                  key={conv.id}
                  initial={{ opacity: 0, x: -8 }}
                  animate={{ opacity: 1, x: 0 }}
                  exit={{ opacity: 0, x: -8 }}
                  transition={{ duration: 0.15 }}
                >
                  {renderItemContent(conv, false)}
                </motion.div>
              ))}
            </AnimatePresence>
            <div className="my-1.5 mx-2 border-t" style={{ borderColor: colors.border }} />
          </>
        )}
        {onReorder ? (
          <Reorder.Group
            axis="y"
            values={unpinned}
            onReorder={handleReorder}
            as="div"
            className="space-y-0.5"
          >
            {unpinned.map(conv => (
              <Reorder.Item
                key={conv.id}
                value={conv}
                as="div"
                initial={{ opacity: 0, x: -8 }}
                animate={{ opacity: 1, x: 0 }}
                exit={{ opacity: 0, x: -8 }}
                transition={{ duration: 0.15 }}
                whileDrag={{
                  scale: 1.02,
                  boxShadow: '0 4px 16px rgba(0,0,0,0.15)',
                  borderRadius: '8px',
                  backgroundColor: colors.bgSecondary,
                  zIndex: 50,
                }}
                style={{ position: 'relative' }}
              >
                {renderItemContent(conv, true)}
              </Reorder.Item>
            ))}
          </Reorder.Group>
        ) : (
          <AnimatePresence mode="popLayout">
            {unpinned.map(conv => (
              <motion.div
                key={conv.id}
                initial={{ opacity: 0, x: -8 }}
                animate={{ opacity: 1, x: 0 }}
                exit={{ opacity: 0, x: -8 }}
                transition={{ duration: 0.15 }}
              >
                {renderItemContent(conv, false)}
              </motion.div>
            ))}
          </AnimatePresence>
        )}
      </div>
    </div>
  );
}
