import React, { createContext, useContext, useState, useCallback, useEffect } from 'react';

interface SidebarContextType {
  collapsed: boolean;
  toggleSidebar: () => void;
  setSidebarCollapsed: (collapsed: boolean) => void;
}

const SidebarContext = createContext<SidebarContextType>({
  collapsed: false,
  toggleSidebar: () => {},
  setSidebarCollapsed: () => {},
});

export function SidebarProvider({ children }: { children: React.ReactNode }) {
  // Remembered per viewer; storage may be unavailable (private mode).
  const [collapsed, setCollapsed] = useState(() => {
    try {
      return localStorage.getItem('sidebar_collapsed') === 'true';
    } catch {
      return false;
    }
  });

  useEffect(() => {
    try {
      localStorage.setItem('sidebar_collapsed', String(collapsed));
    } catch {
      // The choice lasts for this session only.
    }
  }, [collapsed]);

  const toggleSidebar = useCallback(() => {
    setCollapsed(prev => !prev);
  }, []);

  const setSidebarCollapsed = useCallback((value: boolean) => {
    setCollapsed(value);
  }, []);

  // Ctrl+B / Cmd+B to toggle sidebar
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && !e.altKey && !e.shiftKey && e.key.toLowerCase() === 'b') {
        e.preventDefault();
        toggleSidebar();
      }
    };
    window.addEventListener('keydown', handleKeyDown);
    return () => window.removeEventListener('keydown', handleKeyDown);
  }, [toggleSidebar]);

  return (
    <SidebarContext.Provider value={{ collapsed, toggleSidebar, setSidebarCollapsed }}>
      {children}
    </SidebarContext.Provider>
  );
}

export function useSidebar() {
  return useContext(SidebarContext);
}
