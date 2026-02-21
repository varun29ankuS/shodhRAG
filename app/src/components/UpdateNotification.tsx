import React from 'react';
import { useTheme } from '../contexts/ThemeContext';
import { motion, AnimatePresence } from 'framer-motion';
import { Download, X, Sparkles, RefreshCw } from 'lucide-react';
import { Button } from './ui/button';
import { useUpdater } from '../hooks/useUpdater';

export function UpdateNotification() {
  const { colors } = useTheme();
  const {
    updateAvailable,
    updateInfo,
    downloading,
    downloadProgress,
    downloadAndInstall,
    dismissUpdate,
  } = useUpdater();

  if (!updateAvailable || !updateInfo) return null;

  return (
    <AnimatePresence>
      <motion.div
        initial={{ opacity: 0, y: -20 }}
        animate={{ opacity: 1, y: 0 }}
        exit={{ opacity: 0, y: -20 }}
        className="fixed top-4 right-4 z-50 w-96"
      >
        <div
          className="rounded-xl shadow-2xl border-2 overflow-hidden"
          style={{
            background: colors.bg,
            borderColor: colors.primary,
          }}
        >
          {/* Header */}
          <div
            className="p-4 flex items-center justify-between"
            style={{ background: `${colors.primary}15` }}
          >
            <div className="flex items-center gap-3">
              <Sparkles className="w-6 h-6" style={{ color: colors.primary }} />
              <div>
                <h3 className="font-bold text-sm" style={{ color: colors.text }}>
                  Update Available
                </h3>
                <p className="text-xs" style={{ color: colors.textMuted }}>
                  Version {updateInfo.version}
                </p>
              </div>
            </div>
            <button
              onClick={dismissUpdate}
              className="p-1 rounded hover:opacity-70 transition-all"
              disabled={downloading}
            >
              <X className="w-4 h-4" style={{ color: colors.textMuted }} />
            </button>
          </div>

          {/* Body */}
          <div className="p-4">
            {updateInfo.body && (
              <div className="mb-4">
                <p className="text-xs font-medium mb-2" style={{ color: colors.textSecondary }}>
                  What's new:
                </p>
                <div
                  className="text-xs p-3 rounded-lg max-h-32 overflow-y-auto"
                  style={{
                    background: colors.bgSecondary,
                    color: colors.text,
                  }}
                >
                  {updateInfo.body.split('\n').map((line, idx) => (
                    <p key={idx} className="mb-1">
                      {line}
                    </p>
                  ))}
                </div>
              </div>
            )}

            {/* Download Progress */}
            {downloading && (
              <div className="mb-4">
                <div className="flex items-center justify-between mb-2">
                  <span className="text-xs" style={{ color: colors.textMuted }}>
                    Downloading...
                  </span>
                  <span className="text-xs font-medium" style={{ color: colors.primary }}>
                    {Math.round(downloadProgress)}%
                  </span>
                </div>
                <div
                  className="h-2 rounded-full overflow-hidden"
                  style={{ background: colors.bgTertiary }}
                >
                  <motion.div
                    className="h-full"
                    style={{ background: colors.primary }}
                    initial={{ width: 0 }}
                    animate={{ width: `${downloadProgress}%` }}
                    transition={{ duration: 0.3 }}
                  />
                </div>
              </div>
            )}

            {/* Actions */}
            <div className="flex gap-2">
              <Button
                variant="outline"
                size="sm"
                onClick={dismissUpdate}
                disabled={downloading}
                className="flex-1"
              >
                Later
              </Button>
              <Button
                variant="default"
                size="sm"
                onClick={downloadAndInstall}
                disabled={downloading}
                className="flex-1"
              >
                {downloading ? (
                  <>
                    <RefreshCw className="w-4 h-4 mr-2 animate-spin" />
                    Downloading...
                  </>
                ) : (
                  <>
                    <Download className="w-4 h-4 mr-2" />
                    Update Now
                  </>
                )}
              </Button>
            </div>

            {/* Info */}
            <div className="mt-3 pt-3 border-t" style={{ borderColor: colors.border }}>
              <p className="text-xs text-center" style={{ color: colors.textMuted }}>
                Current: {updateInfo.currentVersion} → New: {updateInfo.version}
              </p>
            </div>
          </div>
        </div>
      </motion.div>
    </AnimatePresence>
  );
}

interface ManualUpdateCheckProps {
  onCheck?: () => void;
}

export function ManualUpdateCheck({ onCheck }: ManualUpdateCheckProps) {
  const { colors } = useTheme();
  const { checking, checkForUpdates } = useUpdater();

  const handleCheck = async () => {
    await checkForUpdates(false);
    onCheck?.();
  };

  return (
    <Button
      variant="ghost"
      size="sm"
      onClick={handleCheck}
      disabled={checking}
      className="w-full"
    >
      <RefreshCw className={`w-4 h-4 mr-2 ${checking ? 'animate-spin' : ''}`} />
      {checking ? 'Checking for updates...' : 'Check for Updates'}
    </Button>
  );
}
