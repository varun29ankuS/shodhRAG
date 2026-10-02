/**
 * Map a design token from src/index.css (`--c-<name>`) to a Tailwind colour
 * that supports opacity modifiers.
 */
const token = (name) => `color-mix(in srgb, var(--c-${name}) calc(<alpha-value> * 100%), transparent)`;

/** @type {import('tailwindcss').Config} */
module.exports = {
  darkMode: ["class"],
  content: [
    './pages/**/*.{ts,tsx}',
    './components/**/*.{ts,tsx}',
    './app/**/*.{ts,tsx}',
    './src/**/*.{ts,tsx}',
  ],
  prefix: "",
  theme: {
    container: {
      center: true,
      padding: "2rem",
      screens: {
        "2xl": "1400px",
      },
    },
    extend: {
      colors: {
        // Every semantic colour resolves to a --c-* token in src/index.css.
        // color-mix keeps Tailwind opacity modifiers (bg-primary/90) working
        // while the token itself stays a plain hex value.
        border: token('border'),
        input: token('border-strong'),
        ring: token('accent-text'),
        background: token('ground'),
        foreground: token('text'),
        primary: {
          DEFAULT: token('accent'),
          foreground: token('on-accent'),
        },
        secondary: {
          DEFAULT: token('raised-2'),
          foreground: token('text'),
        },
        destructive: {
          DEFAULT: token('danger'),
          foreground: token('on-danger'),
        },
        muted: {
          DEFAULT: token('raised'),
          foreground: token('text-muted'),
        },
        accent: {
          DEFAULT: token('raised-2'),
          foreground: token('text'),
        },
        popover: {
          DEFAULT: token('raised'),
          foreground: token('text'),
        },
        card: {
          DEFAULT: token('surface-2'),
          foreground: token('text'),
        },
        shodh: {
          ground: token('ground'),
          sidebar: token('sidebar'),
          surface: token('surface'),
          'surface-2': token('surface-2'),
          raised: token('raised'),
          'raised-2': token('raised-2'),
          pressed: token('pressed'),
          'border-subtle': token('border-subtle'),
          border: token('border'),
          'border-strong': token('border-strong'),
          text: token('text'),
          'text-secondary': token('text-secondary'),
          'text-tertiary': token('text-tertiary'),
          'text-muted': token('text-muted'),
          'text-faint': token('text-faint'),
          accent: token('accent'),
          'accent-hover': token('accent-hover'),
          'accent-text': token('accent-text'),
          'accent-soft': token('accent-soft'),
          'on-accent': token('on-accent'),
          success: token('success'),
          'success-soft': token('success-soft'),
          warning: token('warning'),
          'warning-soft': token('warning-soft'),
          info: token('info'),
          violet: token('violet'),
          error: token('error'),
        },
      },
      borderRadius: {
        lg: "var(--radius)",
        md: "calc(var(--radius) - 2px)",
        sm: "calc(var(--radius) - 4px)",
      },
      keyframes: {
        "accordion-down": {
          from: { height: "0" },
          to: { height: "var(--radix-accordion-content-height)" },
        },
        "accordion-up": {
          from: { height: "var(--radix-accordion-content-height)" },
          to: { height: "0" },
        },
        "fade-in": {
          from: { opacity: "0" },
          to: { opacity: "1" },
        },
        "fade-out": {
          from: { opacity: "1" },
          to: { opacity: "0" },
        },
        "slide-in": {
          from: { transform: "translateX(-100%)" },
          to: { transform: "translateX(0)" },
        },
        "slide-out": {
          from: { transform: "translateX(0)" },
          to: { transform: "translateX(-100%)" },
        },
        "fade-in-up": {
          from: {
            opacity: "0",
            transform: "translateY(20px)"
          },
          to: {
            opacity: "1",
            transform: "translateY(0)"
          },
        },
        "scale-in": {
          from: {
            opacity: "0",
            transform: "scale(0.95)"
          },
          to: {
            opacity: "1",
            transform: "scale(1)"
          },
        },
        shimmer: {
          "100%": {
            transform: "translateX(100%)",
          },
        },
      },
      animation: {
        "accordion-down": "accordion-down 0.2s ease-out",
        "accordion-up": "accordion-up 0.2s ease-out",
        "fade-in": "fade-in 0.2s ease-out",
        "fade-out": "fade-out 0.2s ease-out",
        "slide-in": "slide-in 0.3s ease-out",
        "slide-out": "slide-out 0.3s ease-out",
        "fade-in-up": "fade-in-up 0.6s ease-out",
        "scale-in": "scale-in 0.5s ease-out",
        shimmer: "shimmer 2s infinite",
      },
      fontFamily: {
        sans: ['"Geist Variable"', "system-ui", "-apple-system", '"Segoe UI"', "sans-serif"],
        mono: ['"Geist Mono Variable"', "ui-monospace", '"Cascadia Code"', "Consolas", "monospace"],
      },
      transitionDuration: {
        micro: "var(--dur-micro)",
        panel: "var(--dur-panel)",
        screen: "var(--dur-screen)",
      },
      transitionTimingFunction: {
        standard: "var(--ease-standard)",
      },
    },
  },
  plugins: [require("tailwindcss-animate")],
}