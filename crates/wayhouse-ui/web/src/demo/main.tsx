import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { HashRouter, MemoryRouter } from "react-router-dom";
import { installMock } from "./mock";
import { App } from "../App";
import { TooltipProvider } from "../components/ui/Tooltip";
import { ToastProvider } from "../components/ui/Toast";
import { ThemeProvider } from "../lib/theme/provider";
import "../index.css";
import "@fontsource/public-sans/latin-400.css";
import "@fontsource/public-sans/latin-500.css";
import "@fontsource/public-sans/latin-600.css";
import "@fontsource/ibm-plex-mono/latin-400.css";
import "@fontsource/ibm-plex-mono/latin-500.css";
import "@fontsource/ibm-plex-mono/latin-600.css";

// The demo entry (demo.html): same providers as main.tsx, a hash router so it works from a plain
// folder or a single file, and the mock backend installed before the first request.
installMock();

// A page embedded in another site (the single-file build for a hosted preview) keeps its route in memory.
const Router = import.meta.env.VITE_DEMO_MEMORY ? MemoryRouter : HashRouter;

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <ThemeProvider>
      <TooltipProvider>
        <ToastProvider>
          <Router>
            <App />
          </Router>
        </ToastProvider>
      </TooltipProvider>
    </ThemeProvider>
  </StrictMode>,
);
