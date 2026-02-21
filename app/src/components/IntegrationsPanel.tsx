import { useState, useEffect, useRef } from "react";
import { motion, AnimatePresence } from "framer-motion";
import { Button } from "./ui/button";
import { Badge } from "./ui/badge";
import { Zap, Check, Globe, Bot, Info, ChevronUp, X } from "lucide-react";
import { useTheme } from "../contexts/ThemeContext";
import { invoke } from "@tauri-apps/api/core";
import { notify } from "../lib/notify";
import { TelegramBotPanel } from "./TelegramBotPanel";
import { DiscordBotPanel } from "./DiscordBotPanel";
import { WhatsAppBotPanel } from "./WhatsAppBotPanel";
import { GoogleDrivePanel } from "./GoogleDrivePanel";

type IntegrationId = "telegram" | "discord" | "whatsapp" | "google-drive" | "slack" | "teams";

interface IntegrationsPanelProps {
  spaces: Array<{ id: string; name: string }>;
}

export function IntegrationsPanel({ spaces }: IntegrationsPanelProps) {
  const { colors } = useTheme();
  const [expanded, setExpanded] = useState<IntegrationId | null>(null);
  const [isTelegramBotActive, setIsTelegramBotActive] = useState(false);
  const [isDiscordBotActive, setIsDiscordBotActive] = useState(false);
  const [isGoogleDriveConnected, setIsGoogleDriveConnected] = useState(false);
  const detailRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    // Check live backend status instead of trusting localStorage
    const checkTelegram = async () => {
      try {
        const status = await invoke<boolean>("check_telegram_bot_status");
        setIsTelegramBotActive(status);
        if (!status) localStorage.removeItem("telegram_bot_status");
      } catch {
        setIsTelegramBotActive(false);
        localStorage.removeItem("telegram_bot_status");
      }
    };
    checkTelegram();
    const handleTgStatus = (event: any) => setIsTelegramBotActive(event.detail.connected);
    window.addEventListener("telegram-bot-status", handleTgStatus);

    const checkDiscord = async () => {
      try {
        const status = await invoke<boolean>("check_discord_bot_status");
        setIsDiscordBotActive(status);
        if (!status) localStorage.removeItem("discord_bot_status");
      } catch {
        setIsDiscordBotActive(false);
        localStorage.removeItem("discord_bot_status");
      }
    };
    checkDiscord();
    const handleDcStatus = (event: any) => setIsDiscordBotActive(event.detail.connected);
    window.addEventListener("discord-bot-status", handleDcStatus);

    const checkGoogleDrive = async () => {
      try {
        const isConnected = await invoke<boolean>("is_google_drive_authenticated");
        setIsGoogleDriveConnected(isConnected);
      } catch { /* not available */ }
    };
    checkGoogleDrive();

    return () => {
      window.removeEventListener("telegram-bot-status", handleTgStatus);
      window.removeEventListener("discord-bot-status", handleDcStatus);
    };
  }, []);

  // Scroll expanded detail into view
  useEffect(() => {
    if (expanded && detailRef.current) {
      setTimeout(() => detailRef.current?.scrollIntoView({ behavior: "smooth", block: "nearest" }), 100);
    }
  }, [expanded]);

  const activeCount =
    (isTelegramBotActive ? 1 : 0) +
    (isDiscordBotActive ? 1 : 0) +
    (isGoogleDriveConnected ? 1 : 0);

  const handleCardClick = (id: IntegrationId) => {
    if (id === "slack" || id === "teams") {
      notify.info(`${id === 'slack' ? 'Slack' : 'Teams'} integration coming soon`);
      return;
    }
    setExpanded(prev => prev === id ? null : id);
  };

  const isActive = (id: IntegrationId): boolean => {
    switch (id) {
      case "telegram": return isTelegramBotActive;
      case "discord": return isDiscordBotActive;
      case "google-drive": return isGoogleDriveConnected;
      default: return false;
    }
  };

  // SVG Icons
  const WhatsAppIcon = (
    <svg viewBox="0 0 24 24" className="w-5 h-5 fill-white">
      <path d="M17.472 14.382c-.297-.149-1.758-.867-2.03-.967-.273-.099-.471-.148-.67.15-.197.297-.767.966-.94 1.164-.173.199-.347.223-.644.075-.297-.15-1.255-.463-2.39-1.475-.883-.788-1.48-1.761-1.653-2.059-.173-.297-.018-.458.13-.606.134-.133.298-.347.446-.52.149-.174.198-.298.298-.497.099-.198.05-.371-.025-.52-.075-.149-.669-1.612-.916-2.207-.242-.579-.487-.5-.669-.51-.173-.008-.371-.01-.57-.01-.198 0-.52.074-.792.372-.272.297-1.04 1.016-1.04 2.479 0 1.462 1.065 2.875 1.213 3.074.149.198 2.096 3.2 5.077 4.487.709.306 1.262.489 1.694.625.712.227 1.36.195 1.871.118.571-.085 1.758-.719 2.006-1.413.248-.694.248-1.289.173-1.413-.074-.124-.272-.198-.57-.347m-5.421 7.403h-.004a9.87 9.87 0 01-5.031-1.378l-.361-.214-3.741.982.998-3.648-.235-.374a9.86 9.86 0 01-1.51-5.26c.001-5.45 4.436-9.884 9.888-9.884 2.64 0 5.122 1.03 6.988 2.898a9.825 9.825 0 012.893 6.994c-.003 5.45-4.437 9.884-9.885 9.884m8.413-18.297A11.815 11.815 0 0012.05 0C5.495 0 .16 5.335.157 11.892c0 2.096.547 4.142 1.588 5.945L.057 24l6.305-1.654a11.882 11.882 0 005.683 1.448h.005c6.554 0 11.89-5.335 11.893-11.893a11.821 11.821 0 00-3.48-8.413Z"/>
    </svg>
  );
  const TelegramIcon = (
    <svg viewBox="0 0 24 24" className="w-5 h-5 fill-white">
      <path d="M11.944 0A12 12 0 0 0 0 12a12 12 0 0 0 12 12 12 12 0 0 0 12-12A12 12 0 0 0 12 0a12 12 0 0 0-.056 0zm4.962 7.224c.1-.002.321.023.465.14a.506.506 0 0 1 .171.325c.016.093.036.306.02.472-.18 1.898-.962 6.502-1.36 8.627-.168.9-.499 1.201-.82 1.23-.696.065-1.225-.46-1.9-.902-1.056-.693-1.653-1.124-2.678-1.8-1.185-.78-.417-1.21.258-1.91.177-.184 3.247-2.977 3.307-3.23.007-.032.014-.15-.056-.212s-.174-.041-.249-.024c-.106.024-1.793 1.14-5.061 3.345-.48.33-.913.49-1.302.48-.428-.008-1.252-.241-1.865-.44-.752-.245-1.349-.374-1.297-.789.027-.216.325-.437.893-.663 3.498-1.524 5.83-2.529 6.998-3.014 3.332-1.386 4.025-1.627 4.476-1.635z"/>
    </svg>
  );
  const DiscordIcon = (
    <svg viewBox="0 0 24 24" className="w-5 h-5 fill-white">
      <path d="M20.317 4.37a19.791 19.791 0 0 0-4.885-1.515.074.074 0 0 0-.079.037c-.21.375-.444.864-.608 1.25a18.27 18.27 0 0 0-5.487 0 12.64 12.64 0 0 0-.617-1.25.077.077 0 0 0-.079-.037A19.736 19.736 0 0 0 3.677 4.37a.07.07 0 0 0-.032.027C.533 9.046-.32 13.58.099 18.057a.082.082 0 0 0 .031.057 19.9 19.9 0 0 0 5.993 3.03.078.078 0 0 0 .084-.028c.462-.63.874-1.295 1.226-1.994a.076.076 0 0 0-.041-.106 13.107 13.107 0 0 1-1.872-.892.077.077 0 0 1-.008-.128 10.2 10.2 0 0 0 .372-.292.074.074 0 0 1 .077-.01c3.928 1.793 8.18 1.793 12.062 0a.074.074 0 0 1 .078.01c.12.098.246.198.373.292a.077.077 0 0 1-.006.127 12.299 12.299 0 0 1-1.873.892.077.077 0 0 0-.041.107c.36.698.772 1.362 1.225 1.993a.076.076 0 0 0 .084.028 19.839 19.839 0 0 0 6.002-3.03.077.077 0 0 0 .032-.054c.5-5.177-.838-9.674-3.549-13.66a.061.061 0 0 0-.031-.03zM8.02 15.33c-1.183 0-2.157-1.085-2.157-2.419 0-1.333.956-2.419 2.157-2.419 1.21 0 2.176 1.096 2.157 2.42 0 1.333-.956 2.418-2.157 2.418zm7.975 0c-1.183 0-2.157-1.085-2.157-2.419 0-1.333.955-2.419 2.157-2.419 1.21 0 2.176 1.096 2.157 2.42 0 1.333-.946 2.418-2.157 2.418z"/>
    </svg>
  );
  const GoogleDriveLogo = (
    <svg viewBox="0 0 87.3 78" className="w-5 h-5">
      <path fill="#0066DA" d="m6.6 66.85 3.85 6.65c.8 1.4 1.95 2.5 3.3 3.3l13.75-23.8h-27.5c0 1.55.4 3.1 1.2 4.5z"/>
      <path fill="#00AC47" d="m43.65 25-13.75-23.8c-1.35.8-2.5 1.9-3.3 3.3l-25.4 44a9.06 9.06 0 0 0 -1.2 4.5h27.5z"/>
      <path fill="#EA4335" d="m73.55 76.8c1.35-.8 2.5-1.9 3.3-3.3l1.6-2.75 7.65-13.25c.8-1.4 1.2-2.95 1.2-4.5h-27.502l5.852 11.5z"/>
      <path fill="#00832D" d="m43.65 25 13.75-23.8c-1.35-.8-2.9-1.2-4.5-1.2h-18.5c-1.6 0-3.15.45-4.5 1.2z"/>
      <path fill="#2684FC" d="m59.8 53h-32.3l-13.75 23.8c1.35.8 2.9 1.2 4.5 1.2h50.8c1.6 0 3.15-.45 4.5-1.2z"/>
      <path fill="#FFBA00" d="m73.4 26.5-12.7-22c-.8-1.4-1.95-2.5-3.3-3.3l-13.75 23.8 16.15 28h27.45c0-1.55-.4-3.1-1.2-4.5z"/>
    </svg>
  );
  const SlackIcon = (
    <svg viewBox="0 0 24 24" className="w-5 h-5 fill-white">
      <path d="M5.042 15.165a2.528 2.528 0 0 1-2.52 2.523A2.528 2.528 0 0 1 0 15.165a2.527 2.527 0 0 1 2.522-2.52h2.52v2.52zM6.313 15.165a2.527 2.527 0 0 1 2.521-2.52 2.527 2.527 0 0 1 2.521 2.52v6.313A2.528 2.528 0 0 1 8.834 24a2.528 2.528 0 0 1-2.521-2.522v-6.313zM8.834 5.042a2.528 2.528 0 0 1-2.521-2.52A2.528 2.528 0 0 1 8.834 0a2.528 2.528 0 0 1 2.521 2.522v2.52H8.834zM8.834 6.313a2.528 2.528 0 0 1 2.521 2.521 2.528 2.528 0 0 1-2.521 2.521H2.522A2.528 2.528 0 0 1 0 8.834a2.528 2.528 0 0 1 2.522-2.521h6.312zM18.956 8.834a2.528 2.528 0 0 1 2.522-2.521A2.528 2.528 0 0 1 24 8.834a2.528 2.528 0 0 1-2.522 2.521h-2.522V8.834zM17.688 8.834a2.528 2.528 0 0 1-2.523 2.521 2.527 2.527 0 0 1-2.52-2.521V2.522A2.527 2.527 0 0 1 15.165 0a2.528 2.528 0 0 1 2.523 2.522v6.312zM15.165 18.956a2.528 2.528 0 0 1 2.523 2.522A2.528 2.528 0 0 1 15.165 24a2.527 2.527 0 0 1-2.52-2.522v-2.522h2.52zM15.165 17.688a2.527 2.527 0 0 1-2.52-2.523 2.526 2.526 0 0 1 2.52-2.52h6.313A2.527 2.527 0 0 1 24 15.165a2.528 2.528 0 0 1-2.522 2.523h-6.313z"/>
    </svg>
  );
  const TeamsIcon = (
    <svg viewBox="0 0 24 24" className="w-5 h-5 fill-white">
      <path d="M20.625 8.127h-2.976V5.15a5.15 5.15 0 1 0-10.298 0v2.977H4.375A4.377 4.377 0 0 0 0 12.502v7.123A4.377 4.377 0 0 0 4.375 24h16.25A4.377 4.377 0 0 0 25 19.625v-7.123a4.377 4.377 0 0 0-4.375-4.375zM9.476 5.15a3.024 3.024 0 0 1 6.048 0v2.977H9.476zM22.875 19.625c0 1.239-1.011 2.25-2.25 2.25H4.375a2.252 2.252 0 0 1-2.25-2.25v-7.123c0-1.239 1.011-2.25 2.25-2.25h16.25c1.239 0 2.25 1.011 2.25 2.25z"/>
    </svg>
  );

  interface CardDef {
    id: IntegrationId;
    icon: React.ReactNode;
    brandColor: string;
    name: string;
    description: string;
    activeDescription: string;
    status: "active" | "soon";
  }

  const cards: CardDef[] = [
    { id: "telegram", icon: TelegramIcon, brandColor: "#0088cc", name: "Telegram", description: "Chat with your knowledge base via Telegram", activeDescription: "Bot is running and receiving messages", status: "active" },
    { id: "discord", icon: DiscordIcon, brandColor: "#5865F2", name: "Discord", description: "Community server knowledge bot", activeDescription: "Bot is running in your server", status: "active" },
    { id: "whatsapp", icon: WhatsAppIcon, brandColor: "#25D366", name: "WhatsApp", description: "Answer messages with your knowledge base", activeDescription: "WhatsApp bridge is running", status: "active" },
    { id: "google-drive", icon: <div className="flex items-center justify-center">{GoogleDriveLogo}</div>, brandColor: "#fff", name: "Google Drive", description: "Sync files from Google Drive", activeDescription: "Sync folders to spaces automatically", status: "active" },
    { id: "slack", icon: SlackIcon, brandColor: "#4A154B", name: "Slack", description: "Team knowledge in channels", activeDescription: "", status: "soon" },
    { id: "teams", icon: TeamsIcon, brandColor: "#5059C9", name: "Teams", description: "Enterprise workspace bot", activeDescription: "", status: "soon" },
  ];

  const renderDetail = () => {
    switch (expanded) {
      case "telegram": return <TelegramBotPanel />;
      case "discord": return <DiscordBotPanel />;
      case "whatsapp": return <WhatsAppBotPanel />;
      case "google-drive": return <GoogleDrivePanel spaces={spaces} />;
      default: return null;
    }
  };

  const expandedCard = cards.find(c => c.id === expanded);

  return (
    <div className="h-full overflow-y-auto" style={{ backgroundColor: colors.bg }}>
      <div className="p-6 max-w-6xl mx-auto">
        {/* Header */}
        <div className="flex items-center gap-3 mb-6">
          <div className="w-9 h-9 rounded-lg flex items-center justify-center" style={{ backgroundColor: colors.primary }}>
            <Zap className="w-5 h-5 text-white" />
          </div>
          <div>
            <h1 className="text-xl font-bold" style={{ color: colors.text }}>Integrations</h1>
            <p className="text-xs" style={{ color: colors.textMuted }}>
              Connect Shodh to your favorite apps and access your knowledge everywhere
            </p>
          </div>
        </div>

        {/* Status Bar */}
        <div className="mb-4 p-3 rounded-lg flex items-center justify-between"
          style={{ backgroundColor: colors.cardBg, border: `1px solid ${colors.cardBorder}` }}
        >
          <div className="flex gap-4">
            <div className="flex items-center gap-2">
              <div className="w-2 h-2 rounded-full" style={{ backgroundColor: colors.success }} />
              <span className="text-sm" style={{ color: colors.text }}>{activeCount} Active</span>
            </div>
            <div className="flex items-center gap-2">
              <div className="w-2 h-2 rounded-full" style={{ backgroundColor: colors.primary }} />
              <span className="text-sm" style={{ color: colors.textMuted }}>4 Available</span>
            </div>
          </div>
          <div className="flex items-center gap-2 text-xs" style={{ color: colors.textMuted }}>
            <Globe className="w-3 h-3" />
            {isTelegramBotActive && <><span style={{ color: colors.success }}>Telegram</span><span>·</span></>}
            {isDiscordBotActive && <><span style={{ color: colors.success }}>Discord</span><span>·</span></>}
            {isGoogleDriveConnected && <><span style={{ color: colors.success }}>Google Drive</span><span>·</span></>}
            <span>Bot integrations</span>
          </div>
        </div>

        {/* Integration Cards Grid */}
        <div className="grid grid-cols-2 md:grid-cols-3 lg:grid-cols-4 xl:grid-cols-6 gap-3">
          {cards.map((card) => {
            const active = isActive(card.id);
            const isExpanded = expanded === card.id;
            return (
              <motion.div
                key={card.id}
                whileHover={card.status === "active" ? { scale: 1.03 } : {}}
                whileTap={card.status === "active" ? { scale: 0.98 } : {}}
                onClick={() => handleCardClick(card.id)}
                className="p-3 rounded-lg cursor-pointer transition-all"
                style={{
                  backgroundColor: isExpanded ? `${card.brandColor}12` : colors.cardBg,
                  border: `${isExpanded ? "2px" : "1px"} solid ${isExpanded ? card.brandColor : active ? card.brandColor : colors.cardBorder}`,
                }}
              >
                <div className="flex items-start justify-between mb-2">
                  <div className="w-8 h-8 rounded-lg flex items-center justify-center" style={{ backgroundColor: card.brandColor }}>
                    {card.icon}
                  </div>
                  {active ? (
                    <Badge className="text-[8px] px-1 py-0" style={{ backgroundColor: colors.success, color: "#fff" }}>
                      <Check className="w-2 h-2 mr-0.5" />
                      Active
                    </Badge>
                  ) : isExpanded ? (
                    <ChevronUp className="w-4 h-4" style={{ color: colors.textMuted }} />
                  ) : (
                    <Badge variant="outline" className="text-[8px] px-1 py-0"
                      style={{ borderColor: colors.border, color: colors.textMuted }}
                    >
                      {card.status === "active" ? "Ready" : "Soon"}
                    </Badge>
                  )}
                </div>
                <h3 className="text-xs font-semibold mb-0.5" style={{ color: colors.text }}>{card.name}</h3>
                <p className="text-[9px] leading-relaxed" style={{ color: colors.textMuted }}>
                  {active ? card.activeDescription : card.description}
                </p>
              </motion.div>
            );
          })}
        </div>

        {/* Expanded Detail Section */}
        <AnimatePresence mode="wait">
          {expanded && expandedCard && expandedCard.status === "active" && (
            <motion.div
              ref={detailRef}
              key={expanded}
              initial={{ opacity: 0, height: 0, marginTop: 0 }}
              animate={{ opacity: 1, height: "auto", marginTop: 16 }}
              exit={{ opacity: 0, height: 0, marginTop: 0 }}
              transition={{ duration: 0.25, ease: "easeInOut" }}
              className="overflow-hidden"
            >
              <div
                className="rounded-lg border overflow-hidden"
                style={{ borderColor: expandedCard.brandColor, backgroundColor: colors.cardBg }}
              >
                {/* Detail header */}
                <div
                  className="flex items-center justify-between px-4 py-2.5 border-b"
                  style={{ borderColor: colors.border }}
                >
                  <div className="flex items-center gap-2">
                    <div className="w-6 h-6 rounded flex items-center justify-center" style={{ backgroundColor: expandedCard.brandColor }}>
                      {expandedCard.icon}
                    </div>
                    <span className="text-sm font-semibold" style={{ color: colors.text }}>
                      {expandedCard.name}
                    </span>
                  </div>
                  <button
                    onClick={(e) => { e.stopPropagation(); setExpanded(null); }}
                    className="p-1 rounded-md hover:opacity-70 transition-opacity"
                    style={{ color: colors.textMuted }}
                  >
                    <X className="w-4 h-4" />
                  </button>
                </div>

                {/* Detail body — render the sub-panel without its own header/bg */}
                <div className="max-h-[60vh] overflow-y-auto">
                  {renderDetail()}
                </div>
              </div>
            </motion.div>
          )}
        </AnimatePresence>

        {/* Info Card */}
        {!expanded && (
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
                  How Integrations Work
                </h3>
                <div className="space-y-1">
                  <p className="text-xs" style={{ color: colors.textMuted }}>
                    Click any integration card to configure it. Bot integrations (Telegram, Discord, WhatsApp) let people chat with your knowledge base through messaging apps.
                  </p>
                  <p className="text-xs" style={{ color: colors.textMuted }}>
                    Google Drive syncs files directly into your Shodh spaces for automatic indexing and RAG-powered search.
                  </p>
                </div>
              </div>
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
