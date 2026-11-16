import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { LanguageProvider } from "./i18n";
import { ThemeProvider, initThemeFromStorage } from "./theme";
import "./index.css";
import { ConnectionProvider } from "./ConnectionControls";

// Pre-paint the persisted theme before the first render to avoid a flash.
initThemeFromStorage();

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <ThemeProvider>
      <LanguageProvider>
        <ConnectionProvider><App /></ConnectionProvider>
      </LanguageProvider>
    </ThemeProvider>
  </React.StrictMode>,
);
