import type { Config } from "tailwindcss";

export default {
  darkMode: "class",
  content: ["./index.html", "./src/**/*.{ts,tsx}"],
  theme: {
    extend: {
      fontFamily: {
        sans: [
          "Segoe UI Variable",
          "Segoe UI",
          "Leelawadee UI",
          "Noto Sans Thai",
          "system-ui",
          "sans-serif",
        ],
      },
      colors: {
        ds: {
          bg: "var(--ds-bg)",
          "control-border": "var(--ds-control-border)",
          "success": "var(--ds-success)",
          "success-soft": "var(--ds-success-soft)",
          "success-border": "var(--ds-success-border)",
          "warning": "var(--ds-warning)",
          "danger": "var(--ds-danger)",
          "danger-soft": "var(--ds-danger-soft)",
          "danger-border": "var(--ds-danger-border)",
          "info": "var(--ds-info)",
          "info-soft": "var(--ds-info-soft)",
          "info-border": "var(--ds-info-border)",

          surface: "var(--ds-surface)",
          elevated: "var(--ds-elevated)",
          sidebar: "var(--ds-sidebar)",
          accent: "var(--ds-accent)",
          "accent-strong": "var(--ds-accent-strong)",
          "on-accent": "var(--ds-on-accent)",
          "accent-soft": "var(--ds-accent-soft)",
          "accent-border": "var(--ds-accent-border)",
          text: "var(--ds-text)",
          "text-muted": "var(--ds-text-muted)",
          border: "var(--ds-border)",
          hover: "var(--ds-hover)",
        },
      },
    },
  },
  plugins: [],
} satisfies Config;
