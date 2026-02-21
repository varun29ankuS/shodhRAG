import { useState, useEffect } from "react";
import { motion } from "framer-motion";
import { Button } from "./ui/button";
import { Input } from "./ui/input";
import { useTheme } from "../contexts/ThemeContext";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { notify } from "../lib/notify";
import {
  Cloud,
  FolderOpen,
  Check,
  X,
  RefreshCw,
  Settings,
  LogOut,
  Loader2,
  ChevronRight,
  AlertCircle,
  Info
} from "lucide-react";

interface DriveFile {
  id: string;
  name: string;
  mime_type: string;
  size?: number;
  modified_time: string;
  web_view_link?: string;
  is_folder: boolean;
}

interface FolderSyncConfig {
  folder_id: string;
  folder_name: string;
  space_id: string;
  auto_sync: boolean;
  sync_subdirectories: boolean;
}

interface SyncStatus {
  is_syncing: boolean;
  total_files: number;
  synced_files: number;
  failed_files: number;
  last_sync?: string;
  error?: string;
}

interface GoogleDrivePanelProps {
  spaces: Array<{ id: string; name: string }>;
}

const GoogleDriveLogo = () => (
  <svg viewBox="0 0 87.3 78" className="w-5 h-5">
    <path fill="#0066DA" d="m6.6 66.85 3.85 6.65c.8 1.4 1.95 2.5 3.3 3.3l13.75-23.8h-27.5c0 1.55.4 3.1 1.2 4.5z"/>
    <path fill="#00AC47" d="m43.65 25-13.75-23.8c-1.35.8-2.5 1.9-3.3 3.3l-25.4 44a9.06 9.06 0 0 0 -1.2 4.5h27.5z"/>
    <path fill="#EA4335" d="m73.55 76.8c1.35-.8 2.5-1.9 3.3-3.3l1.6-2.75 7.65-13.25c.8-1.4 1.2-2.95 1.2-4.5h-27.502l5.852 11.5z"/>
    <path fill="#00832D" d="m43.65 25 13.75-23.8c-1.35-.8-2.9-1.2-4.5-1.2h-18.5c-1.6 0-3.15.45-4.5 1.2z"/>
    <path fill="#2684FC" d="m59.8 53h-32.3l-13.75 23.8c1.35.8 2.9 1.2 4.5 1.2h50.8c1.6 0 3.15-.45 4.5-1.2z"/>
    <path fill="#FFBA00" d="m73.4 26.5-12.7-22c-.8-1.4-1.95-2.5-3.3-3.3l-13.75 23.8 16.15 28h27.45c0-1.55-.4-3.1-1.2-4.5z"/>
  </svg>
);

export function GoogleDrivePanel({ spaces }: GoogleDrivePanelProps) {
  const { colors } = useTheme();

  const [isAuthenticated, setIsAuthenticated] = useState(false);
  const [clientId, setClientId] = useState("");
  const [clientSecret, setClientSecret] = useState("");
  const [isAuthenticating, setIsAuthenticating] = useState(false);

  const [files, setFiles] = useState<DriveFile[]>([]);
  const [currentFolder, setCurrentFolder] = useState<string | null>(null);
  const [folderPath, setFolderPath] = useState<Array<{ id: string; name: string }>>([]);
  const [isLoadingFiles, setIsLoadingFiles] = useState(false);

  const [syncConfigs, setSyncConfigs] = useState<FolderSyncConfig[]>([]);
  const [selectedFolder, setSelectedFolder] = useState<DriveFile | null>(null);
  const [selectedSpace, setSelectedSpace] = useState<string>("");
  const [syncStatus, setSyncStatus] = useState<SyncStatus | null>(null);

  const [showSetup, setShowSetup] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    checkAuthStatus();
  }, []);

  const checkAuthStatus = async () => {
    try {
      const authenticated = await invoke<boolean>("is_google_drive_authenticated");
      setIsAuthenticated(authenticated);
      if (authenticated) {
        loadFiles(null);
      }
    } catch (err) {
      console.error("Failed to check auth status:", err);
    }
  };

  const handleConnect = async () => {
    if (!clientId || !clientSecret) {
      notify.error("Enter both Client ID and Client Secret");
      return;
    }

    setIsAuthenticating(true);
    setError(null);

    try {
      // Initialize OAuth — backend starts callback server on :3000
      const authUrl = await invoke<string>("init_google_drive_oauth", {
        clientId,
        clientSecret,
      });

      // Open auth URL in browser
      window.open(authUrl, "_blank");
      toast("Authorize in the browser window", {
        description: "Waiting for Google to redirect back...",
        duration: 30000,
      });

      // Listen for the auth code from the backend callback server
      const unlisten = await listen<string>("google-drive-auth-code", async (event) => {
        const code = event.payload;
        try {
          await invoke("exchange_google_drive_code", { code });
          setIsAuthenticated(true);
          setShowSetup(false);
          notify.success("Google Drive connected");
          loadFiles(null);
        } catch (err) {
          notify.error("Failed to exchange auth code", { description: String(err) });
          setError(String(err));
        } finally {
          setIsAuthenticating(false);
          unlisten();
        }
      });

      // Also listen for errors
      const unlistenError = await listen<string>("google-drive-auth-error", (event) => {
        notify.error("Google auth failed", { description: event.payload });
        setError(event.payload);
        setIsAuthenticating(false);
        unlistenError();
        unlisten();
      });

      // Timeout after 5 minutes
      setTimeout(() => {
        if (isAuthenticating) {
          setIsAuthenticating(false);
          setError("Authorization timed out. Please try again.");
          notify.error("Authorization timed out");
          unlisten();
          unlistenError();
        }
      }, 300000);
    } catch (err: any) {
      notify.error("Failed to start OAuth", { description: String(err) });
      setError(String(err));
      setIsAuthenticating(false);
    }
  };

  const handleDisconnect = async () => {
    try {
      await invoke("disconnect_google_drive");
      setIsAuthenticated(false);
      setFiles([]);
      setSyncConfigs([]);
      setSelectedFolder(null);
      notify.success("Google Drive disconnected");
    } catch (err: any) {
      notify.error("Failed to disconnect", { description: String(err) });
    }
  };

  const loadFiles = async (folderId: string | null) => {
    setIsLoadingFiles(true);
    setError(null);

    try {
      const fileList = await invoke<DriveFile[]>("list_google_drive_files", { folderId });
      setFiles(fileList);
      setCurrentFolder(folderId);
    } catch (err: any) {
      notify.error("Failed to load files", { description: String(err) });
    } finally {
      setIsLoadingFiles(false);
    }
  };

  const handleFolderClick = (folder: DriveFile) => {
    setFolderPath([...folderPath, { id: folder.id, name: folder.name }]);
    loadFiles(folder.id);
  };

  const handleBreadcrumbClick = (index: number) => {
    if (index === -1) {
      setFolderPath([]);
      loadFiles(null);
    } else {
      const newPath = folderPath.slice(0, index + 1);
      setFolderPath(newPath);
      loadFiles(newPath[newPath.length - 1].id);
    }
  };

  const handleConfigureSync = async () => {
    if (!selectedFolder || !selectedSpace) {
      notify.error("Select both a folder and a space");
      return;
    }

    try {
      const config: FolderSyncConfig = {
        folder_id: selectedFolder.id,
        folder_name: selectedFolder.name,
        space_id: selectedSpace,
        auto_sync: true,
        sync_subdirectories: false,
      };

      await invoke("configure_folder_sync", { config });
      setSyncConfigs([...syncConfigs, config]);
      setSelectedFolder(null);
      setSelectedSpace("");
      notify.success(`Sync configured for ${config.folder_name}`);
    } catch (err: any) {
      notify.error("Failed to configure sync", { description: String(err) });
    }
  };

  const handleSyncNow = async (config: FolderSyncConfig) => {
    try {
      toast("Syncing files...", { description: config.folder_name });
      const status = await invoke<SyncStatus>("sync_google_drive_folder", {
        folderId: config.folder_id,
        spaceId: config.space_id,
      });
      setSyncStatus(status);
      if (!status.error) {
        notify.success(`Synced ${status.synced_files} files`);
      }
    } catch (err: any) {
      notify.error("Sync failed", { description: String(err) });
    }
  };

  const formatFileSize = (bytes?: number) => {
    if (!bytes) return "-";
    if (bytes < 1024) return bytes + " B";
    if (bytes < 1024 * 1024) return (bytes / 1024).toFixed(1) + " KB";
    return (bytes / (1024 * 1024)).toFixed(1) + " MB";
  };

  // ── Unauthenticated view ──
  if (!isAuthenticated) {
    return (
      <div className="h-full flex items-center justify-center p-6" style={{ backgroundColor: colors.bg }}>
        <div className="max-w-md w-full">
          {!showSetup ? (
            <motion.div
              initial={{ opacity: 0, y: 20 }}
              animate={{ opacity: 1, y: 0 }}
              className="text-center"
            >
              <div className="w-16 h-16 mx-auto mb-4 rounded-lg flex items-center justify-center"
                style={{ backgroundColor: colors.cardBg, border: `1px solid ${colors.border}` }}
              >
                <svg viewBox="0 0 87.3 78" className="w-10 h-10">
                  <path fill="#0066DA" d="m6.6 66.85 3.85 6.65c.8 1.4 1.95 2.5 3.3 3.3l13.75-23.8h-27.5c0 1.55.4 3.1 1.2 4.5z"/>
                  <path fill="#00AC47" d="m43.65 25-13.75-23.8c-1.35.8-2.5 1.9-3.3 3.3l-25.4 44a9.06 9.06 0 0 0 -1.2 4.5h27.5z"/>
                  <path fill="#EA4335" d="m73.55 76.8c1.35-.8 2.5-1.9 3.3-3.3l1.6-2.75 7.65-13.25c.8-1.4 1.2-2.95 1.2-4.5h-27.502l5.852 11.5z"/>
                  <path fill="#00832D" d="m43.65 25 13.75-23.8c-1.35-.8-2.9-1.2-4.5-1.2h-18.5c-1.6 0-3.15.45-4.5 1.2z"/>
                  <path fill="#2684FC" d="m59.8 53h-32.3l-13.75 23.8c1.35.8 2.9 1.2 4.5 1.2h50.8c1.6 0 3.15-.45 4.5-1.2z"/>
                  <path fill="#FFBA00" d="m73.4 26.5-12.7-22c-.8-1.4-1.95-2.5-3.3-3.3l-13.75 23.8 16.15 28h27.45c0-1.55-.4-3.1-1.2-4.5z"/>
                </svg>
              </div>
              <h2 className="text-xl font-bold mb-2" style={{ color: colors.text }}>
                Connect Google Drive
              </h2>
              <p className="text-sm mb-6" style={{ color: colors.textMuted }}>
                Sync your files from Google Drive to Shodh spaces automatically
              </p>
              <Button
                onClick={() => setShowSetup(true)}
                className="w-full"
                style={{ backgroundColor: "#4285F4", color: "#fff" }}
              >
                <Cloud className="w-4 h-4 mr-2" />
                Connect to Google Drive
              </Button>
            </motion.div>
          ) : (
            <motion.div
              initial={{ opacity: 0, y: 20 }}
              animate={{ opacity: 1, y: 0 }}
              className="p-6 rounded-lg"
              style={{ backgroundColor: colors.cardBg, border: `1px solid ${colors.border}` }}
            >
              <div className="flex items-center justify-between mb-4">
                <h3 className="text-base font-semibold" style={{ color: colors.text }}>
                  Google Drive Setup
                </h3>
                <Button variant="ghost" size="sm" onClick={() => setShowSetup(false)}>
                  <X className="w-4 h-4" />
                </Button>
              </div>

              <div className="space-y-4">
                <div>
                  <label className="block text-sm font-medium mb-2" style={{ color: colors.text }}>
                    Client ID
                  </label>
                  <Input
                    value={clientId}
                    onChange={(e) => setClientId(e.target.value)}
                    placeholder="Your Google OAuth Client ID"
                    className="w-full"
                    style={{ backgroundColor: colors.inputBg, borderColor: colors.border, color: colors.text }}
                  />
                </div>

                <div>
                  <label className="block text-sm font-medium mb-2" style={{ color: colors.text }}>
                    Client Secret
                  </label>
                  <Input
                    value={clientSecret}
                    onChange={(e) => setClientSecret(e.target.value)}
                    placeholder="Your Google OAuth Client Secret"
                    type="password"
                    className="w-full"
                    style={{ backgroundColor: colors.inputBg, borderColor: colors.border, color: colors.text }}
                  />
                </div>

                {error && (
                  <div className="p-3 rounded-lg flex items-start gap-2"
                    style={{ backgroundColor: colors.bgTertiary, border: `1px solid ${colors.error}` }}
                  >
                    <AlertCircle className="w-4 h-4 mt-0.5" style={{ color: colors.error }} />
                    <p className="text-sm" style={{ color: colors.error }}>{error}</p>
                  </div>
                )}

                <div className="p-3 rounded-lg" style={{ backgroundColor: colors.bgTertiary }}>
                  <p className="text-xs" style={{ color: colors.textMuted }}>
                    <strong>Need credentials?</strong> Go to{" "}
                    <a
                      href="https://console.cloud.google.com"
                      target="_blank"
                      rel="noopener noreferrer"
                      style={{ color: colors.primary }}
                      className="underline"
                    >
                      Google Cloud Console
                    </a>
                    {" "} → Create OAuth 2.0 credentials → Desktop app
                  </p>
                </div>

                {isAuthenticating && (
                  <div className="p-3 rounded-lg flex items-center gap-2"
                    style={{ backgroundColor: colors.bgTertiary, border: `1px solid ${colors.primary}` }}
                  >
                    <Loader2 className="w-4 h-4 animate-spin" style={{ color: colors.primary }} />
                    <p className="text-sm" style={{ color: colors.text }}>
                      Waiting for authorization in browser...
                    </p>
                  </div>
                )}

                <Button
                  onClick={handleConnect}
                  disabled={isAuthenticating}
                  className="w-full"
                  style={{ backgroundColor: "#4285F4", color: "#fff" }}
                >
                  {isAuthenticating ? (
                    <>
                      <Loader2 className="w-4 h-4 mr-2 animate-spin" />
                      Waiting for auth...
                    </>
                  ) : (
                    <>
                      <Cloud className="w-4 h-4 mr-2" />
                      Connect
                    </>
                  )}
                </Button>
              </div>
            </motion.div>
          )}
        </div>
      </div>
    );
  }

  // ── Authenticated view ──
  return (
    <div className="h-full overflow-y-auto" style={{ backgroundColor: colors.bg }}>
      <div className="p-6 max-w-3xl mx-auto">
        {/* Header */}
        <div className="flex items-center justify-between mb-6">
          <div className="flex items-center gap-3">
            <div className="w-9 h-9 rounded-lg flex items-center justify-center"
              style={{ backgroundColor: colors.cardBg, border: `1px solid ${colors.border}` }}
            >
              <GoogleDriveLogo />
            </div>
            <div>
              <h1 className="text-xl font-bold" style={{ color: colors.text }}>
                Google Drive
              </h1>
              <p className="text-xs" style={{ color: colors.textMuted }}>
                Manage folder sync and download files to your spaces
              </p>
            </div>
          </div>
          <Button variant="outline" size="sm" onClick={handleDisconnect}
            style={{ borderColor: colors.border, color: colors.text }}
          >
            <LogOut className="w-3 h-3 mr-1" />
            <span className="text-xs">Disconnect</span>
          </Button>
        </div>

        {/* Sync Configurations */}
        {syncConfigs.length > 0 && (
          <div className="mb-4">
            <h3 className="text-sm font-semibold mb-2" style={{ color: colors.text }}>
              Configured Syncs
            </h3>
            <div className="space-y-2">
              {syncConfigs.map((config, index) => (
                <div
                  key={index}
                  className="p-3 rounded-lg flex items-center justify-between"
                  style={{ backgroundColor: colors.cardBg, border: `1px solid ${colors.border}` }}
                >
                  <div className="flex items-center gap-3">
                    <FolderOpen className="w-4 h-4" style={{ color: "#4285F4" }} />
                    <div>
                      <p className="text-sm font-medium" style={{ color: colors.text }}>
                        {config.folder_name}
                      </p>
                      <p className="text-xs" style={{ color: colors.textMuted }}>
                        → {spaces.find((s) => s.id === config.space_id)?.name || config.space_id}
                      </p>
                    </div>
                  </div>
                  <Button size="sm" onClick={() => handleSyncNow(config)}
                    style={{ backgroundColor: "#4285F4", color: "#fff" }}
                  >
                    <RefreshCw className="w-3 h-3 mr-1" />
                    <span className="text-xs">Sync</span>
                  </Button>
                </div>
              ))}
            </div>
          </div>
        )}

        {/* Sync Status */}
        {syncStatus && syncStatus.is_syncing && (
          <div className="mb-4 p-3 rounded-lg"
            style={{ backgroundColor: colors.cardBg, border: `1px solid #4285F4` }}
          >
            <div className="flex items-center gap-3 mb-2">
              <Loader2 className="w-4 h-4 animate-spin" style={{ color: "#4285F4" }} />
              <div>
                <p className="text-sm font-medium" style={{ color: colors.text }}>
                  Syncing files...
                </p>
                <p className="text-xs" style={{ color: colors.textMuted }}>
                  {syncStatus.synced_files} / {syncStatus.total_files} files processed
                </p>
              </div>
            </div>
            <div className="w-full rounded-full h-1.5" style={{ backgroundColor: colors.border }}>
              <div
                className="h-1.5 rounded-full transition-all"
                style={{
                  backgroundColor: "#4285F4",
                  width: `${syncStatus.total_files > 0 ? (syncStatus.synced_files / syncStatus.total_files) * 100 : 0}%`,
                }}
              />
            </div>
          </div>
        )}

        {/* File Browser */}
        <div className="rounded-lg" style={{ backgroundColor: colors.cardBg, border: `1px solid ${colors.border}` }}>
          <div className="p-3 border-b" style={{ borderColor: colors.border }}>
            <div className="flex items-center justify-between">
              <div className="flex items-center gap-1.5 text-xs">
                <button
                  onClick={() => handleBreadcrumbClick(-1)}
                  className="hover:underline"
                  style={{ color: colors.primary }}
                >
                  My Drive
                </button>
                {folderPath.map((folder, index) => (
                  <div key={folder.id} className="flex items-center gap-1.5">
                    <ChevronRight className="w-3 h-3" style={{ color: colors.textMuted }} />
                    <button
                      onClick={() => handleBreadcrumbClick(index)}
                      className="hover:underline"
                      style={{ color: colors.primary }}
                    >
                      {folder.name}
                    </button>
                  </div>
                ))}
              </div>
              <Button variant="ghost" size="sm" onClick={() => loadFiles(currentFolder)} disabled={isLoadingFiles}
                className="h-6 px-2"
              >
                {isLoadingFiles ? (
                  <Loader2 className="w-3 h-3 animate-spin" />
                ) : (
                  <RefreshCw className="w-3 h-3" />
                )}
              </Button>
            </div>
          </div>

          <div className="p-3">
            {isLoadingFiles ? (
              <div className="flex items-center justify-center py-12">
                <Loader2 className="w-6 h-6 animate-spin" style={{ color: colors.textMuted }} />
              </div>
            ) : files.length === 0 ? (
              <div className="text-center py-12">
                <FolderOpen className="w-10 h-10 mx-auto mb-2" style={{ color: colors.textMuted }} />
                <p className="text-sm" style={{ color: colors.textMuted }}>No files or folders</p>
              </div>
            ) : (
              <div className="space-y-1">
                {files.map((file) => (
                  <div
                    key={file.id}
                    className="p-2 rounded-lg cursor-pointer transition-all"
                    style={{
                      backgroundColor: selectedFolder?.id === file.id ? colors.primary + "18" : "transparent",
                    }}
                    onClick={() => {
                      if (file.is_folder) {
                        handleFolderClick(file);
                      } else {
                        setSelectedFolder(file);
                      }
                    }}
                  >
                    <div className="flex items-center justify-between">
                      <div className="flex items-center gap-2">
                        {file.is_folder ? (
                          <FolderOpen className="w-4 h-4" style={{ color: "#4285F4" }} />
                        ) : (
                          <svg className="w-4 h-4" viewBox="0 0 24 24" fill={colors.textMuted}>
                            <path d="M14,2H6A2,2 0 0,0 4,4V20A2,2 0 0,0 6,22H18A2,2 0 0,0 20,20V8L14,2M18,20H6V4H13V9H18V20Z" />
                          </svg>
                        )}
                        <div>
                          <p className="text-sm font-medium" style={{ color: colors.text }}>
                            {file.name}
                          </p>
                          <p className="text-[10px]" style={{ color: colors.textMuted }}>
                            {formatFileSize(file.size)} · {new Date(file.modified_time).toLocaleDateString()}
                          </p>
                        </div>
                      </div>
                      {selectedFolder?.id === file.id && !file.is_folder && (
                        <Check className="w-4 h-4" style={{ color: colors.success }} />
                      )}
                    </div>
                  </div>
                ))}
              </div>
            )}
          </div>
        </div>

        {/* Configure Sync */}
        {selectedFolder && selectedFolder.is_folder && (
          <div className="mt-4 p-4 rounded-lg"
            style={{ backgroundColor: colors.cardBg, border: `1px solid #4285F4` }}
          >
            <h3 className="text-sm font-semibold mb-3" style={{ color: colors.text }}>
              Configure Folder Sync
            </h3>
            <div className="flex items-end gap-3">
              <div className="flex-1">
                <label className="block text-xs font-medium mb-1" style={{ color: colors.textMuted }}>
                  Selected Folder
                </label>
                <Input value={selectedFolder.name} disabled className="w-full"
                  style={{ backgroundColor: colors.bgTertiary, borderColor: colors.border, color: colors.text }}
                />
              </div>
              <div className="flex-1">
                <label className="block text-xs font-medium mb-1" style={{ color: colors.textMuted }}>
                  Sync to Space
                </label>
                <select
                  value={selectedSpace}
                  onChange={(e) => setSelectedSpace(e.target.value)}
                  className="w-full px-3 py-2 rounded-md border text-sm"
                  style={{ backgroundColor: colors.inputBg, borderColor: colors.border, color: colors.text }}
                >
                  <option value="">Select a space...</option>
                  {spaces.map((space) => (
                    <option key={space.id} value={space.id}>
                      {space.name}
                    </option>
                  ))}
                </select>
              </div>
              <Button onClick={handleConfigureSync} disabled={!selectedSpace}
                style={{ backgroundColor: "#4285F4", color: "#fff" }}
              >
                <Settings className="w-3 h-3 mr-1" />
                <span className="text-xs">Configure</span>
              </Button>
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
