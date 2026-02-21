import { useState, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { motion } from "framer-motion";
import { Button } from "./ui/button";
import { Badge } from "./ui/badge";
import { Input } from "./ui/input";
import { notify } from "../lib/notify";
import { CheckCircle, ExternalLink, Loader2, Power, RefreshCw, Info, Bot } from "lucide-react";
import { useTheme } from "../contexts/ThemeContext";

export interface BotConfig {
  /** Display name e.g. "Telegram" */
  name: string;
  /** Brand color for accent (icon bg, active border) */
  brandColor: string;
  /** SVG icon component rendered inside the brand-colored circle */
  icon: React.ReactNode;
  /** Tauri command names */
  commands: {
    start: string;
    stop: string;
    checkStatus?: string;
  };
  /** localStorage key prefix */
  storagePrefix: string;
  /** CustomEvent name for cross-component status updates */
  statusEvent: string;
  /** Input placeholder for bot token */
  tokenPlaceholder: string;
  /** Setup instructions rendered as steps */
  setupSteps: { title: string; description: string }[];
  /** URL for the "Open Portal" button */
  portalUrl: string;
  /** Label for the portal button */
  portalLabel: string;
  /** "How It Works" bullet points */
  howItWorks: string[];
}

interface BotPanelProps {
  config: BotConfig;
}

export function BotPanel({ config }: BotPanelProps) {
  const { colors } = useTheme();
  const [botToken, setBotToken] = useState("");
  const [isConnected, setIsConnected] = useState(false);
  const [connectionStatus, setConnectionStatus] = useState("Disconnected");
  const [isStarting, setIsStarting] = useState(false);
  const [isChecking, setIsChecking] = useState(false);

  const checkBotStatus = async () => {
    if (!config.commands.checkStatus) return;
    setIsChecking(true);
    try {
      const status = await invoke(config.commands.checkStatus) as boolean;
      setIsConnected(status);
      setConnectionStatus(status
        ? `Bot is running! Send a message to your ${config.name} bot.`
        : "Disconnected"
      );
    } catch (error) {
      console.error(`Failed to check ${config.name} bot status:`, error);
    } finally {
      setIsChecking(false);
    }
  };

  useEffect(() => {
    const savedToken = localStorage.getItem(`${config.storagePrefix}_token`);
    if (savedToken) setBotToken(savedToken);

    if (config.commands.checkStatus) {
      checkBotStatus();
    } else {
      const savedStatus = localStorage.getItem(`${config.storagePrefix}_status`);
      if (savedStatus === 'connected') {
        setIsConnected(true);
        setConnectionStatus(`Bot is running! Send a message in your ${config.name} server.`);
      }
    }
  }, []);

  const handleConnect = async () => {
    if (!botToken.trim()) {
      notify.error(`Enter your ${config.name} bot token`);
      return;
    }

    setIsStarting(true);
    setConnectionStatus("Installing dependencies...");

    try {
      await invoke(config.commands.start, { token: botToken });

      setIsConnected(true);
      setConnectionStatus(`Bot is running! Send a message to your ${config.name} bot.`);
      localStorage.setItem(`${config.storagePrefix}_token`, botToken);
      localStorage.setItem(`${config.storagePrefix}_status`, 'connected');
      window.dispatchEvent(new CustomEvent(config.statusEvent, { detail: { connected: true } }));

      if (config.commands.checkStatus) await checkBotStatus();
      notify.success(`${config.name} bot started`);
    } catch (error) {
      setConnectionStatus("Failed to start bot");
      notify.error(`Failed to start ${config.name} bot`, { description: String(error) });
      setIsConnected(false);
      localStorage.removeItem(`${config.storagePrefix}_status`);
    } finally {
      setIsStarting(false);
    }
  };

  const handleDisconnect = async () => {
    try {
      await invoke(config.commands.stop);
      setIsConnected(false);
      setConnectionStatus("Disconnected");
      localStorage.removeItem(`${config.storagePrefix}_status`);
      window.dispatchEvent(new CustomEvent(config.statusEvent, { detail: { connected: false } }));
      if (config.commands.checkStatus) await checkBotStatus();
      notify.success(`${config.name} bot stopped`);
    } catch (error) {
      notify.error(`Failed to stop bot: ${error}`);
    }
  };

  return (
    <div className="h-full overflow-y-auto" style={{ backgroundColor: colors.bg }}>
      <div className="p-6 max-w-3xl mx-auto">
        {/* Header */}
        <div className="flex items-center gap-3 mb-6">
          <div className="w-9 h-9 rounded-lg flex items-center justify-center" style={{ backgroundColor: config.brandColor }}>
            {config.icon}
          </div>
          <div>
            <h1 className="text-xl font-bold" style={{ color: colors.text }}>
              {config.name} Bot
            </h1>
            <p className="text-xs" style={{ color: colors.textMuted }}>
              Chat with your knowledge base through {config.name}
            </p>
          </div>
        </div>

        {/* Status bar */}
        <div className="mb-4 p-3 rounded-lg flex items-center justify-between"
          style={{ backgroundColor: colors.cardBg, border: `1px solid ${colors.cardBorder}` }}
        >
          <div className="flex items-center gap-2">
            <div className="w-2 h-2 rounded-full" style={{ backgroundColor: isConnected ? colors.success : colors.textMuted }} />
            <span className="text-sm" style={{ color: colors.text }}>{connectionStatus}</span>
          </div>
          <div className="flex items-center gap-2">
            {config.commands.checkStatus && (
              <Button onClick={checkBotStatus} disabled={isChecking} size="sm" variant="ghost" className="h-6 px-2">
                <RefreshCw className={`w-3 h-3 ${isChecking ? 'animate-spin' : ''}`} />
              </Button>
            )}
            {isConnected && (
              <Button onClick={handleDisconnect} size="sm" variant="ghost" className="h-6 px-2"
                style={{ color: colors.error }}
              >
                <Power className="w-3 h-3 mr-1" />
                <span className="text-xs">Stop</span>
              </Button>
            )}
          </div>
        </div>

        <div className="space-y-4">
          {/* Step 1: Setup instructions */}
          <div className="p-4 rounded-lg" style={{ backgroundColor: colors.cardBg, border: `1px solid ${colors.border}` }}>
            <div className="flex items-start justify-between mb-3">
              <div>
                <h3 className="text-base font-semibold mb-1" style={{ color: colors.text }}>
                  Step 1: Create {config.name} Bot
                </h3>
                <p className="text-xs" style={{ color: colors.textMuted }}>
                  Get your bot token from {config.name}
                </p>
              </div>
              <Badge variant="outline" className="text-[10px] px-1.5 py-0"
                style={{ borderColor: colors.border, color: colors.textMuted }}
              >
                Required
              </Badge>
            </div>

            <div className="space-y-1.5 mb-3">
              {config.setupSteps.map((step, i) => (
                <p key={i} className="text-xs" style={{ color: colors.textMuted }}>
                  {i + 1}. {step.description}
                </p>
              ))}
            </div>

            <Button onClick={() => window.open(config.portalUrl, "_blank")} size="sm" variant="outline" className="w-full"
              style={{ borderColor: colors.border, color: colors.text }}
            >
              <ExternalLink className="w-3 h-3 mr-1" />
              <span className="text-xs">{config.portalLabel}</span>
            </Button>
          </div>

          {/* Step 2: Enter token */}
          <div className="p-4 rounded-lg" style={{ backgroundColor: colors.cardBg, border: `1px solid ${colors.border}` }}>
            <h3 className="text-base font-semibold mb-3" style={{ color: colors.text }}>
              Step 2: Enter Bot Token
            </h3>

            <Input
              type="password"
              value={botToken}
              onChange={e => setBotToken(e.target.value)}
              placeholder={config.tokenPlaceholder}
              className="mb-3"
              style={{ backgroundColor: colors.inputBg, borderColor: colors.border, color: colors.text }}
            />

            <Button
              onClick={handleConnect}
              disabled={!botToken.trim() || isStarting}
              className="w-full"
              style={{ backgroundColor: config.brandColor, color: '#fff' }}
            >
              {isStarting ? (
                <>
                  <Loader2 className="w-3 h-3 mr-1 animate-spin" />
                  <span className="text-xs">Starting...</span>
                </>
              ) : (
                <>
                  <CheckCircle className="w-3 h-3 mr-1" />
                  <span className="text-xs">Start Bot</span>
                </>
              )}
            </Button>
          </div>

          {/* Step 3: Bot running */}
          {isConnected && (
            <motion.div
              initial={{ opacity: 0, y: 12 }}
              animate={{ opacity: 1, y: 0 }}
              className="p-4 rounded-lg"
              style={{ backgroundColor: colors.cardBg, border: `1px solid ${config.brandColor}` }}
            >
              <div className="flex items-center gap-2 mb-2">
                <CheckCircle className="w-4 h-4" style={{ color: colors.success }} />
                <h3 className="text-base font-semibold" style={{ color: colors.text }}>
                  Bot is Running
                </h3>
              </div>

              <p className="text-sm mb-3" style={{ color: colors.textSecondary }}>
                Your {config.name} bot is active and connected to Shodh's RAG system.
              </p>

              <div className="p-3 rounded-lg mb-3"
                style={{ backgroundColor: colors.bgTertiary, border: `1px solid ${colors.border}` }}
              >
                <p className="text-xs font-medium mb-1" style={{ color: colors.text }}>Next Steps:</p>
                <p className="text-xs" style={{ color: colors.textMuted }}>
                  1. Open {config.name} and find your bot<br />
                  2. Send a message to start chatting<br />
                  3. The bot answers from your knowledge base
                </p>
              </div>

              <Button onClick={handleDisconnect} className="w-full" variant="outline"
                style={{ borderColor: colors.border, color: colors.text }}
              >
                <Power className="w-3 h-3 mr-1" />
                <span className="text-xs">Stop Bot</span>
              </Button>
            </motion.div>
          )}
        </div>

        {/* How It Works */}
        <div className="mt-6 p-4 rounded-lg"
          style={{ backgroundColor: colors.bgTertiary, border: `1px solid ${colors.border}` }}
        >
          <div className="flex items-start gap-3">
            <div className="w-7 h-7 rounded-full flex items-center justify-center flex-shrink-0"
              style={{ backgroundColor: colors.buttonBg }}
            >
              <Info className="w-3.5 h-3.5" style={{ color: colors.primary }} />
            </div>
            <div>
              <h3 className="text-sm font-semibold mb-2" style={{ color: colors.text }}>
                How It Works
              </h3>
              <div className="space-y-1">
                {config.howItWorks.map((item, i) => (
                  <p key={i} className="text-xs" style={{ color: colors.textMuted }}>{item}</p>
                ))}
              </div>
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}
