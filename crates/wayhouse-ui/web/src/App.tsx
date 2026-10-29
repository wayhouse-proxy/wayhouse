import { lazy, useEffect, useState } from "react";
import { Navigate, Route, Routes } from "react-router-dom";
import { getSession, logout, type SessionInfo } from "./api";
import { Login } from "./components/Login";
import { Shell } from "./components/shell/Shell";
import { Loading } from "./components/shell/States";
import { SessionProvider } from "./lib/session";
import { QueryProvider } from "./lib/query";
import { ThemeSync } from "./lib/uidata";
import { PoolsEditorPage, ResolversEditorPage, SourcesEditorPage, WebEditorPage } from "./pages/ConfigListPages";
import { ListenersEditorPage } from "./pages/ListenersEditorPage";
import { FleetPage } from "./pages/FleetPage";
import { DashboardPage } from "./pages/DashboardPage";

import { SettingsPage } from "./pages/SettingsPage";
import { SourcePage } from "./pages/SourcePage";
import { SharedConfigDoc } from "./lib/config/SharedConfigDoc";
import { TrialBar } from "./components/config/TrialBar";
import { AppearancePage } from "./pages/AppearancePage";
import { LayoutPage } from "./pages/LayoutPage";
import { SniffersPage } from "./pages/SniffersPage";
import { PluginsPage } from "./pages/PluginsPage";
import { PluginInstallPage } from "./pages/PluginInstallPage";
import { PluginDetailPage } from "./pages/PluginDetailPage";
import { ConfigHistoryPage } from "./pages/ConfigHistoryPage";
import { TunnelAddressesPage } from "./pages/TunnelAddressesPage";
import { CertificatesPage } from "./pages/CertificatesPage";
import { FeaturesPage } from "./pages/FeaturesPage";

// The chart library is the heaviest dependency, so the pages that use it load on first visit.
const OverviewPage = lazy(() => import("./pages/OverviewPage").then((m) => ({ default: m.OverviewPage })));
const ListenersPage = lazy(() => import("./pages/ListenersPage").then((m) => ({ default: m.ListenersPage })));
const PoolsPage = lazy(() => import("./pages/PoolsPage").then((m) => ({ default: m.PoolsPage })));

export function App() {
  // undefined while checking, null when signed out
  const [session, setSession] = useState<SessionInfo | null | undefined>(undefined);

  useEffect(() => {
    getSession().then(setSession);
  }, []);

  if (session === undefined) {
    return (
      <div className="p-(--page-pad)">
        <Loading rows={1} />
      </div>
    );
  }
  if (!session) {
    return <Login onLoggedIn={() => getSession().then(setSession)} />;
  }

  return (
    <SessionProvider value={session}>
    <QueryProvider>
    <ThemeSync />
    <Routes>
      <Route element={<Shell banner={<TrialBar />} onSignOut={() => logout().finally(() => setSession(null))} />}>
        <Route index element={<Navigate to="/dashboard" replace />} />
        <Route path="/dashboard" element={<DashboardPage />} />
        <Route path="/overview" element={<OverviewPage />} />
        <Route path="/listeners" element={<ListenersPage />} />
        <Route path="/pools" element={<PoolsPage />} />
        <Route path="/fleet" element={<FleetPage />} />
        <Route element={<SharedConfigDoc />}>
          <Route path="/settings" element={<SettingsPage />} />
          <Route path="/settings/source" element={<SourcePage />} />
          <Route path="/config/listeners" element={<ListenersEditorPage />} />
          <Route path="/config/pools" element={<PoolsEditorPage />} />
          <Route path="/config/sources" element={<SourcesEditorPage />} />
          <Route path="/config/resolvers" element={<ResolversEditorPage />} />
          <Route path="/config/web" element={<WebEditorPage />} />
        </Route>
        <Route path="/settings/appearance" element={<AppearancePage />} />
        <Route path="/settings/layout" element={<LayoutPage />} />
        <Route path="/sniffers" element={<SniffersPage />} />
        <Route path="/plugins" element={<PluginsPage />} />
        <Route path="/plugins/new" element={<PluginInstallPage />} />
        <Route path="/plugins/:id" element={<PluginDetailPage />} />
        <Route path="/config-history" element={<ConfigHistoryPage />} />
        <Route path="/tunnel" element={<TunnelAddressesPage />} />
        <Route path="/certificates" element={<CertificatesPage />} />
        <Route path="/features" element={<FeaturesPage />} />
        <Route path="*" element={<Navigate to="/dashboard" replace />} />
      </Route>
    </Routes>
    </QueryProvider>
    </SessionProvider>
  );
}
