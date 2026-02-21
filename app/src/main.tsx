import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App-SplitView";
import DailyBriefWindow from "./DailyBrief";
import MapViewWindow from "./MapView";
import { ThemeProvider, useTheme } from "./contexts/ThemeContext";
import { SidebarProvider } from "./contexts/SidebarContext";
import { PermissionProvider } from "./contexts/PermissionContext";
import ErrorBoundary from "./components/ErrorBoundary";
import { Toaster } from "sonner";
import { initErrorReporting } from "./lib/errorReporting";

import "./index.css";

initErrorReporting();

const path = window.location.pathname;
let Component = App;

if (path === '/daily-brief') {
  Component = DailyBriefWindow;
} else if (path === '/map-view') {
  Component = MapViewWindow;
}

function ThemedToaster() {
  const { theme, colors } = useTheme();
  return (
    <Toaster
      theme={theme}
      position="bottom-right"
      expand={false}
      richColors
      closeButton
      toastOptions={{
        style: {
          borderRadius: '10px',
          fontSize: '12px',
          padding: '12px 16px',
          boxShadow: theme === 'dark'
            ? '0 8px 24px rgba(0,0,0,0.4)'
            : '0 8px 24px rgba(0,0,0,0.12)',
          backgroundColor: colors.cardBg,
          color: colors.text,
          border: `1px solid ${colors.border}`,
        },
        className: 'shodh-toast',
      }}
      gap={8}
      visibleToasts={4}
      duration={3500}
      offset={16}
    />
  );
}

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <ErrorBoundary>
      <ThemeProvider>
        <SidebarProvider>
        <PermissionProvider>
          <Component />
          <ThemedToaster />
        </PermissionProvider>
        </SidebarProvider>
      </ThemeProvider>
    </ErrorBoundary>
  </React.StrictMode>,
);