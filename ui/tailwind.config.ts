import type { Config } from "tailwindcss";

export default {
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
        // Loose Windows 11 Settings palette
        accent: {
          DEFAULT: "#0078d4",
          hover: "#106ebe",
        },
      },
    },
  },
  plugins: [],
} satisfies Config;
