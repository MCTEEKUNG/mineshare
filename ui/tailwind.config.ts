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
          bg: "#0F172A",
          surface: "#1B2336",
          elevated: "#1E293B",
          sidebar: "#0D1525",
          accent: "#22C55E",
        },
      },
    },
  },
  plugins: [],
} satisfies Config;
