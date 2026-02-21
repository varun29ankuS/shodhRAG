import { useState, useEffect, useRef } from "react";
import { motion } from "framer-motion";
import { invoke } from "@tauri-apps/api/core";
import { Button } from "./ui/button";
import { Badge } from "./ui/badge";
import { Input } from "./ui/input";
import { notify } from "../lib/notify";
import {
  Bot,
  MessageSquare,
  Sparkles,
  CheckCircle,
  Power,
  Send,
  Zap,
  Info,
  Loader2,
  Smartphone,
} from "lucide-react";
import { useTheme } from "../contexts/ThemeContext";

interface BotStats {
  total_contacts: number;
  authorized_contacts: number;
  total_conversations: number;
  total_messages: number;
  total_responses: number;
  active: boolean;
}

interface BotResponse {
  message: string;
  sources: string[];
  confidence: number;
  used_space: string | null;
}

type WhatsAppEngine = "baileys" | "webjs";

export function WhatsAppBotPanel() {
  const { colors } = useTheme();
  const [isConnected, setIsConnected] = useState(false);
  const [ragEnabled, setRagEnabled] = useState(false);
  const [stats, setStats] = useState<BotStats | null>(null);
  const [engine, setEngine] = useState<WhatsAppEngine>("baileys");
  const [isConnecting, setIsConnecting] = useState(false);

  const [testMessage, setTestMessage] = useState("");
  const [testResponse, setTestResponse] = useState<BotResponse | null>(null);
  const [isLoading, setIsLoading] = useState(false);
  const [qrDataUrl, setQrDataUrl] = useState<string | null>(null);
  const [qrStatus, setQrStatus] = useState<"idle" | "initializing" | "waiting_scan" | "connected">("idle");
  const qrPollRef = useRef<ReturnType<typeof setInterval> | null>(null);

  const failCountRef = useRef(0);

  const stopQrPolling = () => {
    if (qrPollRef.current) {
      clearInterval(qrPollRef.current);
      qrPollRef.current = null;
    }
    failCountRef.current = 0;
  };

  const startQrPolling = () => {
    stopQrPolling();
    setQrStatus("initializing");
    failCountRef.current = 0;

    qrPollRef.current = setInterval(async () => {
      try {
        const res = await fetch("http://localhost:3457/qr");
        if (res.ok) {
          failCountRef.current = 0;
          const data = await res.json();
          setQrStatus(data.status);
          if (data.qr) {
            setQrDataUrl(data.qr);
          }
          if (data.status === "connected") {
            setIsConnected(true);
            setQrDataUrl(null);
            stopQrPolling();
            checkConnection();
          }
        } else {
          failCountRef.current++;
        }
      } catch {
        failCountRef.current++;
      }

      // If bridge stopped responding for 10+ polls (20s), give up
      if (failCountRef.current >= 10) {
        stopQrPolling();
        setQrStatus("idle");
        setQrDataUrl(null);
        setIsConnecting(false);
        notify.error("WhatsApp bridge stopped", { description: "The bridge process is no longer running. Try connecting again." });
      }
    }, 2000);
  };

  useEffect(() => {
    checkConnection();
    const savedEngine = localStorage.getItem("whatsapp_engine") as WhatsAppEngine;
    if (savedEngine) setEngine(savedEngine);

    return () => {
      stopQrPolling();
    };
  }, []);

  const checkConnection = async () => {
    try {
      const statsData = await invoke<BotStats>("whatsapp_get_stats");
      setStats(statsData);
      setIsConnected(statsData.active);
      setRagEnabled(statsData.active);
    } catch (error) {
      console.error("Failed to check connection:", error);
    }
  };

  const handleConnectWhatsApp = async () => {
    setIsConnecting(true);
    try {
      const result = await invoke<string>("whatsapp_initialize", {
        botPhone: "+1234567890",
        engine,
      });
      setIsConnected(true);
      localStorage.setItem("whatsapp_engine", engine);

      notify.success("WhatsApp bridge starting", {
        description: "QR code will appear below. Scan it with WhatsApp > Settings > Linked Devices.",
        duration: 5000,
      });

      // Start polling for QR code from the bridge
      startQrPolling();
      checkConnection();
    } catch (error) {
      notify.error("Failed to connect WhatsApp", { description: String(error) });
    } finally {
      setIsConnecting(false);
    }
  };

  const handleToggleRagChat = async () => {
    try {
      const newState = !ragEnabled;
      await invoke("whatsapp_set_active", { active: newState });
      setRagEnabled(newState);
      notify.success(newState ? "RAG Chat enabled" : "RAG Chat disabled");
      checkConnection();
    } catch (error) {
      notify.error("Failed to toggle RAG chat", { description: String(error) });
    }
  };

  const handleTestMessage = async () => {
    if (!testMessage.trim()) return;

    setIsLoading(true);
    try {
      const response = await invoke<BotResponse>("whatsapp_test_message", { message: testMessage });
      setTestResponse(response);
    } catch (error) {
      notify.error("Test message failed", { description: String(error) });
      setTestResponse({ message: `Error: ${error}`, sources: [], confidence: 0, used_space: null });
    } finally {
      setIsLoading(false);
    }
  };

  return (
    <div className="h-full overflow-y-auto" style={{ backgroundColor: colors.bg }}>
      <div className="p-6 max-w-3xl mx-auto">
        {/* Header */}
        <div className="flex items-center gap-3 mb-6">
          <div className="w-9 h-9 rounded-lg flex items-center justify-center" style={{ backgroundColor: "#25D366" }}>
            <MessageSquare className="w-5 h-5 text-white" />
          </div>
          <div>
            <h1 className="text-xl font-bold" style={{ color: colors.text }}>
              WhatsApp RAG Bot
            </h1>
            <p className="text-xs" style={{ color: colors.textMuted }}>
              Answer WhatsApp messages automatically using your knowledge base
            </p>
          </div>
        </div>

        {/* Status Bar */}
        {stats && (
          <div className="mb-4 p-3 rounded-lg flex items-center justify-between"
            style={{ backgroundColor: colors.cardBg, border: `1px solid ${colors.cardBorder}` }}
          >
            <div className="flex gap-4">
              <div className="flex items-center gap-2">
                <div className="w-2 h-2 rounded-full" style={{ backgroundColor: isConnected ? colors.success : colors.textMuted }} />
                <span className="text-sm" style={{ color: colors.text }}>
                  {isConnected ? "Connected" : "Disconnected"}
                </span>
              </div>
              <div className="flex items-center gap-2">
                <Sparkles className="w-3 h-3" style={{ color: ragEnabled ? colors.success : colors.textMuted }} />
                <span className="text-sm" style={{ color: colors.textMuted }}>
                  RAG {ragEnabled ? "Active" : "Inactive"}
                </span>
              </div>
              <div className="flex items-center gap-2">
                <MessageSquare className="w-3 h-3" style={{ color: colors.textMuted }} />
                <span className="text-sm" style={{ color: colors.textMuted }}>
                  {stats.total_messages} messages
                </span>
              </div>
            </div>
          </div>
        )}

        {/* Engine Selection */}
        {!isConnected && (
          <div className="mb-4 p-4 rounded-lg"
            style={{ backgroundColor: colors.cardBg, border: `1px solid ${colors.border}` }}
          >
            <h3 className="text-sm font-semibold mb-3" style={{ color: colors.text }}>
              Connection Method
            </h3>
            <div className="grid grid-cols-2 gap-3">
              <button
                onClick={() => setEngine("baileys")}
                className="p-3 rounded-lg text-left transition-all"
                style={{
                  backgroundColor: engine === "baileys" ? `${colors.primary}14` : colors.bgTertiary,
                  border: `1px solid ${engine === "baileys" ? colors.primary : colors.border}`,
                }}
              >
                <div className="flex items-center gap-2 mb-1">
                  <Zap className="w-3.5 h-3.5" style={{ color: engine === "baileys" ? colors.primary : colors.textMuted }} />
                  <span className="text-sm font-medium" style={{ color: colors.text }}>
                    Baileys
                  </span>
                  <Badge className="text-[8px] px-1 py-0" style={{ backgroundColor: colors.success, color: "#fff" }}>
                    Recommended
                  </Badge>
                </div>
                <p className="text-[10px] leading-relaxed" style={{ color: colors.textMuted }}>
                  Lightweight WebSocket connection. No browser needed. Faster startup, lower memory. Uses multi-device API directly.
                </p>
              </button>
              <button
                onClick={() => setEngine("webjs")}
                className="p-3 rounded-lg text-left transition-all"
                style={{
                  backgroundColor: engine === "webjs" ? `${colors.primary}14` : colors.bgTertiary,
                  border: `1px solid ${engine === "webjs" ? colors.primary : colors.border}`,
                }}
              >
                <div className="flex items-center gap-2 mb-1">
                  <Bot className="w-3.5 h-3.5" style={{ color: engine === "webjs" ? colors.primary : colors.textMuted }} />
                  <span className="text-sm font-medium" style={{ color: colors.text }}>
                    WhatsApp Web.js
                  </span>
                </div>
                <p className="text-[10px] leading-relaxed" style={{ color: colors.textMuted }}>
                  Puppeteer-based browser automation. More stable for long sessions. Requires Chromium (~200MB). Well-tested library.
                </p>
              </button>
            </div>
          </div>
        )}

        {/* Action Cards */}
        <div className="grid grid-cols-1 md:grid-cols-2 gap-4 mb-4">
          {/* Step 1: Connect */}
          <div
            className="p-4 rounded-lg transition-all"
            style={{
              backgroundColor: colors.cardBg,
              border: `1px solid ${isConnected ? "#25D366" : colors.cardBorder}`,
            }}
          >
            <div className="flex items-start justify-between mb-3">
              <div className="w-10 h-10 rounded-lg flex items-center justify-center"
                style={{ backgroundColor: isConnected ? "#25D366" : colors.primary }}
              >
                <MessageSquare className="w-5 h-5 text-white" />
              </div>
              {isConnected ? (
                <Badge className="text-[10px] px-1.5 py-0" style={{ backgroundColor: colors.success, color: "#fff" }}>
                  <CheckCircle className="w-2.5 h-2.5 mr-0.5" />
                  Connected
                </Badge>
              ) : (
                <Badge variant="outline" className="text-[10px] px-1.5 py-0"
                  style={{ borderColor: colors.border, color: colors.textMuted }}
                >
                  Step 1
                </Badge>
              )}
            </div>
            <h3 className="text-base font-semibold mb-1" style={{ color: colors.text }}>
              {isConnected ? "WhatsApp Connected" : "Connect WhatsApp"}
            </h3>
            <p className="text-xs mb-3" style={{ color: colors.textMuted }}>
              {isConnected
                ? `Connected via ${engine === "baileys" ? "Baileys" : "WhatsApp Web.js"}`
                : `Will connect using ${engine === "baileys" ? "Baileys (lightweight)" : "WhatsApp Web.js (Puppeteer)"}`}
            </p>
            {!isConnected && qrStatus === "idle" && (
              <Button
                className="w-full"
                size="sm"
                onClick={handleConnectWhatsApp}
                disabled={isConnecting}
                style={{ backgroundColor: "#25D366", color: "#fff" }}
              >
                {isConnecting ? (
                  <>
                    <div className="w-3 h-3 border-2 border-white border-t-transparent rounded-full animate-spin mr-1" />
                    <span className="text-xs">Starting bridge...</span>
                  </>
                ) : (
                  <>
                    <Zap className="w-3 h-3 mr-1" />
                    <span className="text-xs">Connect Now</span>
                  </>
                )}
              </Button>
            )}

            {/* Inline QR Code */}
            {!isConnected && qrStatus !== "idle" && (
              <div className="mt-3">
                {qrStatus === "initializing" && !qrDataUrl && (
                  <div className="flex flex-col items-center gap-2 py-4">
                    <Loader2 className="w-6 h-6 animate-spin" style={{ color: "#25D366" }} />
                    <span className="text-xs" style={{ color: colors.textMuted }}>Starting bridge, waiting for QR...</span>
                  </div>
                )}
                {qrDataUrl && (
                  <motion.div
                    initial={{ opacity: 0, scale: 0.9 }}
                    animate={{ opacity: 1, scale: 1 }}
                    className="flex flex-col items-center gap-3"
                  >
                    <div className="p-3 rounded-lg bg-white">
                      <img src={qrDataUrl} alt="WhatsApp QR Code" className="w-56 h-56" />
                    </div>
                    <div className="flex items-center gap-2">
                      <Smartphone className="w-3.5 h-3.5" style={{ color: "#25D366" }} />
                      <span className="text-xs font-medium" style={{ color: colors.text }}>
                        Scan with WhatsApp
                      </span>
                    </div>
                    <p className="text-[10px] text-center" style={{ color: colors.textMuted }}>
                      Open WhatsApp → Settings → Linked Devices → Link a Device
                    </p>
                  </motion.div>
                )}
              </div>
            )}
          </div>

          {/* Step 2: Enable RAG */}
          <div
            className="p-4 rounded-lg cursor-pointer transition-all"
            style={{
              backgroundColor: colors.cardBg,
              border: `1px solid ${ragEnabled ? colors.success : colors.cardBorder}`,
              opacity: isConnected ? 1 : 0.5,
            }}
            onClick={isConnected ? handleToggleRagChat : undefined}
          >
            <div className="flex items-start justify-between mb-3">
              <div className="w-10 h-10 rounded-lg flex items-center justify-center"
                style={{ backgroundColor: ragEnabled ? colors.success : (isConnected ? colors.primary : colors.cardBorder) }}
              >
                <Sparkles className="w-5 h-5 text-white" />
              </div>
              {ragEnabled ? (
                <Badge className="text-[10px] px-1.5 py-0" style={{ backgroundColor: colors.success, color: "#fff" }}>
                  <CheckCircle className="w-2.5 h-2.5 mr-0.5" />
                  Active
                </Badge>
              ) : (
                <Badge variant="outline" className="text-[10px] px-1.5 py-0"
                  style={{ borderColor: colors.border, color: colors.textMuted }}
                >
                  Step 2
                </Badge>
              )}
            </div>
            <h3 className="text-base font-semibold mb-1" style={{ color: isConnected ? colors.text : colors.textMuted }}>
              {ragEnabled ? "RAG Chat Active" : "Enable RAG Chat"}
            </h3>
            <p className="text-xs mb-3" style={{ color: colors.textMuted }}>
              {ragEnabled ? "Auto-replying with your knowledge base" : isConnected ? "Click to enable automatic replies" : "Connect WhatsApp first"}
            </p>
            {isConnected && (
              <Button className="w-full" size="sm"
                style={ragEnabled ? { backgroundColor: colors.error, color: "#fff" } : { backgroundColor: colors.success, color: "#fff" }}
              >
                <Power className="w-3 h-3 mr-1" />
                <span className="text-xs">{ragEnabled ? "Disable" : "Enable Now"}</span>
              </Button>
            )}
          </div>
        </div>

        {/* Test Message */}
        {isConnected && ragEnabled && (
          <motion.div
            initial={{ opacity: 0, y: 12 }}
            animate={{ opacity: 1, y: 0 }}
            className="p-4 rounded-lg mb-4"
            style={{ backgroundColor: colors.cardBg, border: `1px solid ${colors.cardBorder}` }}
          >
            <h3 className="text-sm font-semibold mb-3" style={{ color: colors.text }}>
              Test Your Bot
            </h3>
            <div className="flex gap-2 mb-3">
              <Input
                value={testMessage}
                onChange={(e) => setTestMessage(e.target.value)}
                placeholder="Ask anything from your knowledge base..."
                onKeyDown={(e) => e.key === "Enter" && !isLoading && handleTestMessage()}
                disabled={isLoading}
                style={{ backgroundColor: colors.inputBg, borderColor: colors.border, color: colors.text }}
                className="text-sm"
              />
              <Button
                onClick={handleTestMessage}
                disabled={isLoading || !testMessage.trim()}
                size="sm"
                style={{ backgroundColor: colors.success, color: "#fff" }}
              >
                {isLoading ? (
                  <div className="w-4 h-4 border-2 border-white border-t-transparent rounded-full animate-spin" />
                ) : (
                  <>
                    <Send className="w-3 h-3 mr-1" />
                    <span className="text-xs">Send</span>
                  </>
                )}
              </Button>
            </div>

            {testResponse && (
              <motion.div
                initial={{ opacity: 0 }}
                animate={{ opacity: 1 }}
                className="p-3 rounded-lg"
                style={{ backgroundColor: colors.bgTertiary, border: `1px solid ${colors.border}` }}
              >
                <p className="text-xs font-semibold mb-2" style={{ color: colors.success }}>
                  Bot Reply:
                </p>
                <p className="text-sm mb-2" style={{ color: colors.text }}>
                  {testResponse.message}
                </p>
                {testResponse.sources && testResponse.sources.length > 0 && (
                  <p className="text-[10px]" style={{ color: colors.textMuted }}>
                    Sources: {testResponse.sources.join(", ")}
                  </p>
                )}
              </motion.div>
            )}
          </motion.div>
        )}

        {/* How It Works */}
        <div className="p-4 rounded-lg"
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
                <p className="text-xs" style={{ color: colors.textMuted }}>
                  1. Choose a connection method — Baileys (fast, no browser) or WhatsApp Web.js (Puppeteer)
                </p>
                <p className="text-xs" style={{ color: colors.textMuted }}>
                  2. Click "Connect Now" — the app starts the WhatsApp bridge automatically
                </p>
                <p className="text-xs" style={{ color: colors.textMuted }}>
                  3. Scan the QR code with WhatsApp (Settings → Linked Devices → Link a Device)
                </p>
                <p className="text-xs" style={{ color: colors.textMuted }}>
                  4. Enable RAG Chat — incoming messages are answered from your knowledge base
                </p>
              </div>

              <div className="mt-3 p-2.5 rounded-lg" style={{ backgroundColor: colors.cardBg, border: `1px solid ${colors.border}` }}>
                <p className="text-[10px] font-medium mb-1" style={{ color: colors.text }}>Baileys vs WhatsApp Web.js</p>
                <div className="space-y-0.5">
                  <p className="text-[10px]" style={{ color: colors.textMuted }}>
                    <strong>Baileys:</strong> Connects via WebSocket directly. No Chromium needed. ~5MB memory. Faster startup. Can disconnect if WhatsApp updates protocol.
                  </p>
                  <p className="text-[10px]" style={{ color: colors.textMuted }}>
                    <strong>Web.js:</strong> Runs headless Chromium browser. ~200MB memory. More stable long-term. Mimics real WhatsApp Web session.
                  </p>
                </div>
              </div>
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}
