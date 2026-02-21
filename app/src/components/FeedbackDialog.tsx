import React, { useState } from 'react';
import { useTheme } from '../contexts/ThemeContext';
import { Bug, MessageSquare, Send, X, CheckCircle } from 'lucide-react';
import { notify } from '../lib/notify';
import { getAppInfo } from '../lib/version';
import { captureError, addBreadcrumb } from '../lib/errorReporting';

interface FeedbackDialogProps {
  isOpen: boolean;
  onClose: () => void;
  type?: 'bug' | 'feedback';
}

export function FeedbackDialog({ isOpen, onClose, type = 'feedback' }: FeedbackDialogProps) {
  const { colors } = useTheme();
  const [feedbackType, setFeedbackType] = useState<'bug' | 'feedback' | 'feature'>(type);
  const [title, setTitle] = useState('');
  const [description, setDescription] = useState('');
  const [email, setEmail] = useState('');
  const [isSubmitting, setIsSubmitting] = useState(false);
  const [submitted, setSubmitted] = useState(false);

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();

    if (!title.trim() || !description.trim()) {
      notify.error('Please fill in all required fields');
      return;
    }

    setIsSubmitting(true);
    addBreadcrumb('Feedback submission started', 'feedback', { type: feedbackType });

    try {
      const appInfo = getAppInfo();
      const feedbackData = {
        type: feedbackType,
        title: title.trim(),
        description: description.trim(),
        email: email.trim() || 'anonymous',
        appInfo,
        timestamp: new Date().toISOString(),
      };

      const subject = encodeURIComponent(`[${feedbackType.toUpperCase()}] ${title}`);
      const body = encodeURIComponent(`
Type: ${feedbackType}
Title: ${title}

Description:
${description}

${email ? `Contact: ${email}\n` : ''}
---
App Version: ${appInfo.version}
Platform: ${appInfo.platform}
User Agent: ${appInfo.userAgent}
      `.trim());

      const mailtoLink = `mailto:beta@kalki.app?subject=${subject}&body=${body}`;
      window.open(mailtoLink);

      console.log('Feedback data:', feedbackData);
      addBreadcrumb('Feedback submitted', 'feedback', { type: feedbackType });

      setSubmitted(true);
      notify.success('Thank you for your feedback!');

      setTimeout(() => {
        onClose();
        setSubmitted(false);
        setTitle('');
        setDescription('');
        setEmail('');
      }, 2000);
    } catch (error) {
      console.error('Failed to submit feedback:', error);
      captureError(error as Error, { context: 'feedback_submission' });
      notify.error('Failed to submit feedback. Please try again.');
    } finally {
      setIsSubmitting(false);
    }
  };

  if (!isOpen) return null;

  if (submitted) {
    return (
      <div className="fixed inset-0 z-50 flex items-center justify-center" style={{ background: 'rgba(0, 0, 0, 0.5)' }} onClick={onClose}>
        <div className="rounded-xl shadow-2xl border-2 p-8 max-w-md w-full text-center" style={{ background: colors.bg, borderColor: colors.border }}>
          <CheckCircle className="w-16 h-16 mx-auto mb-4" style={{ color: colors.success }} />
          <h2 className="text-2xl font-bold mb-2" style={{ color: colors.text }}>
            Thank You!
          </h2>
          <p style={{ color: colors.textMuted }}>
            Your feedback has been sent. We'll review it and get back to you if needed.
          </p>
        </div>
      </div>
    );
  }

  return (
    <>
      <div className="fixed inset-0 z-50" style={{ background: 'rgba(0, 0, 0, 0.5)' }} onClick={onClose} />

      <div className="fixed top-1/2 left-1/2 transform -translate-x-1/2 -translate-y-1/2 z-50 w-full max-w-2xl">
        <div className="rounded-xl shadow-2xl border-2 overflow-hidden" style={{ background: colors.bg, borderColor: colors.border }}>
          <div className="flex items-center justify-between p-6 border-b" style={{ background: colors.bgSecondary, borderColor: colors.border }}>
            <div className="flex items-center gap-3">
              {feedbackType === 'bug' ? (
                <Bug className="w-6 h-6" style={{ color: colors.error }} />
              ) : (
                <MessageSquare className="w-6 h-6" style={{ color: colors.primary }} />
              )}
              <h2 className="text-xl font-bold" style={{ color: colors.text }}>
                {feedbackType === 'bug' ? 'Report a Bug' : feedbackType === 'feature' ? 'Request a Feature' : 'Send Feedback'}
              </h2>
            </div>
            <button
              onClick={onClose}
              className="p-2 rounded-lg transition-all hover:opacity-70"
              style={{ background: colors.bgTertiary }}
            >
              <X className="w-5 h-5" style={{ color: colors.text }} />
            </button>
          </div>

          <form onSubmit={handleSubmit} className="p-6 space-y-6">
            <div className="flex gap-3">
              {(['bug', 'feedback', 'feature'] as const).map((t) => (
                <button
                  key={t}
                  type="button"
                  onClick={() => setFeedbackType(t)}
                  className="flex-1 py-2 px-4 rounded-lg border-2 transition-all text-sm font-medium"
                  style={{
                    background: feedbackType === t ? colors.primary : colors.bgSecondary,
                    borderColor: feedbackType === t ? colors.primary : colors.border,
                    color: feedbackType === t ? '#ffffff' : colors.text,
                  }}
                >
                  {t === 'bug' ? 'Bug' : t === 'feature' ? 'Feature' : 'Feedback'}
                </button>
              ))}
            </div>

            <div>
              <label className="block text-sm font-medium mb-2" style={{ color: colors.text }}>
                Title <span style={{ color: colors.error }}>*</span>
              </label>
              <input
                type="text"
                value={title}
                onChange={(e) => setTitle(e.target.value)}
                placeholder={feedbackType === 'bug' ? 'Brief description of the bug' : 'What would you like to share?'}
                className="w-full px-4 py-3 rounded-lg border-2 transition-all focus:outline-none focus:ring-2"
                style={{
                  background: colors.inputBg,
                  borderColor: colors.border,
                  color: colors.text,
                }}
                required
              />
            </div>

            <div>
              <label className="block text-sm font-medium mb-2" style={{ color: colors.text }}>
                Description <span style={{ color: colors.error }}>*</span>
              </label>
              <textarea
                value={description}
                onChange={(e) => setDescription(e.target.value)}
                placeholder={feedbackType === 'bug' ? 'Steps to reproduce, expected vs actual behavior...' : 'Tell us more...'}
                rows={6}
                className="w-full px-4 py-3 rounded-lg border-2 transition-all focus:outline-none focus:ring-2 resize-none"
                style={{
                  background: colors.inputBg,
                  borderColor: colors.border,
                  color: colors.text,
                }}
                required
              />
            </div>

            <div>
              <label className="block text-sm font-medium mb-2" style={{ color: colors.text }}>
                Email (optional)
              </label>
              <input
                type="email"
                value={email}
                onChange={(e) => setEmail(e.target.value)}
                placeholder="your.email@example.com"
                className="w-full px-4 py-3 rounded-lg border-2 transition-all focus:outline-none focus:ring-2"
                style={{
                  background: colors.inputBg,
                  borderColor: colors.border,
                  color: colors.text,
                }}
              />
              <p className="text-xs mt-1" style={{ color: colors.textMuted }}>
                We'll only use this to follow up on your {feedbackType}
              </p>
            </div>

            <div className="flex gap-3 justify-end pt-4 border-t" style={{ borderColor: colors.border }}>
              <button
                type="button"
                onClick={onClose}
                className="px-6 py-3 rounded-lg font-medium transition-all border-2"
                style={{
                  background: 'transparent',
                  borderColor: colors.border,
                  color: colors.text,
                }}
              >
                Cancel
              </button>
              <button
                type="submit"
                disabled={isSubmitting || !title.trim() || !description.trim()}
                className="px-6 py-3 rounded-lg font-medium transition-all flex items-center gap-2 disabled:opacity-50"
                style={{
                  background: colors.primary,
                  color: '#ffffff',
                }}
              >
                {isSubmitting ? (
                  <>Sending...</>
                ) : (
                  <>
                    <Send className="w-4 h-4" />
                    Send {feedbackType === 'bug' ? 'Bug Report' : feedbackType === 'feature' ? 'Feature Request' : 'Feedback'}
                  </>
                )}
              </button>
            </div>
          </form>
        </div>
      </div>
    </>
  );
}
