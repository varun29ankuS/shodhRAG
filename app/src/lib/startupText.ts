/**
 * The wording of Settings → General's start-at-login switch for the system Shodh runs
 * on (`background.rs::set_start_with_windows` adds a Windows Run entry, a macOS launch
 * agent, or a Linux autostart entry). No imports: unit-tested under Node.
 */

export type DesktopPlatform = 'windows' | 'macos' | 'linux';

/** The platform from `navigator.platform` ("Win32", "MacIntel", "Linux x86_64"). */
export function desktopPlatform(navigatorPlatform: string): DesktopPlatform {
  if (/^win/i.test(navigatorPlatform)) return 'windows';
  if (/^mac/i.test(navigatorPlatform)) return 'macos';
  return 'linux';
}

export interface StartupText {
  label: string;
  description: string;
}

/** Label and help of the start-at-login switch on `platform`. */
export function startupText(platform: DesktopPlatform): StartupText {
  if (platform === 'windows') {
    return {
      label: 'Start Shodh when Windows starts',
      description:
        'Starts hidden in the tray so reminders ring after a restart. Windows can also turn this off in Task Manager → Startup apps.',
    };
  }
  if (platform === 'macos') {
    return {
      label: 'Open Shodh when you log in',
      description:
        'Opens hidden in the menu bar so reminders ring after a restart. macOS can also turn this off in System Settings → General → Login Items.',
    };
  }
  return {
    label: 'Start Shodh when you log in',
    description: 'Starts hidden in the tray so reminders ring after a restart.',
  };
}
