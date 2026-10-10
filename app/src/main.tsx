import "@fontsource-variable/geist";
import "@fontsource-variable/geist-mono";
import "./index.css";

import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App-SplitView";
import PrintView from "./features/print/PrintView";
import { ThemeProvider, useTheme } from "./contexts/ThemeContext";
import { SidebarProvider } from "./contexts/SidebarContext";
import { SearchModelsProvider } from "./features/setup/SearchModelsContext";
import ErrorBoundary from "./components/ErrorBoundary";
import { Toaster } from "sonner";
import { MotionConfig } from "framer-motion";
import { initErrorReporting } from "./lib/errorReporting";
import { markStartup } from "./lib/startupTiming";

markStartup("modules-evaluated");

initErrorReporting();

const path = window.location.pathname;

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

const root = ReactDOM.createRoot(document.getElementById("root") as HTMLElement);

if (path === '/print-view') {
  // A print window (PDF export): always light, no app shell.
  root.render(
    <React.StrictMode>
      <ErrorBoundary>
        <ThemeProvider forced="light">
          <PrintView />
        </ThemeProvider>
      </ErrorBoundary>
    </React.StrictMode>,
  );
} else {
  root.render(
    <React.StrictMode>
      {/* Every framer-motion animation follows prefers-reduced-motion. */}
      <MotionConfig reducedMotion="user">
        <ErrorBoundary>
          <ThemeProvider>
            <SidebarProvider>
              <SearchModelsProvider>
                <App />
              </SearchModelsProvider>
              <ThemedToaster />
            </SidebarProvider>
          </ThemeProvider>
        </ErrorBoundary>
      </MotionConfig>
    </React.StrictMode>,
  );
}
