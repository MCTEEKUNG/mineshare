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
          "Inter",
          "system-ui",
          "sans-serif",
        ],
      },
      colors: {
        ds: {
          bg: "var(--ds-bg)",
          surface: "var(--ds-surface)",
          elevated: "var(--ds-elevated)",
          sidebar: "var(--ds-sidebar)",
          accent: "var(--ds-accent)",
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
