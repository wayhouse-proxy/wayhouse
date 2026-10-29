import { render } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { SessionProvider, type Role } from "../lib/session";
import { SharedConfigDoc } from "../lib/config/SharedConfigDoc";

/** A page that edits the configuration, inside the layout route that shares its document. */
export function renderConfigPage(path: string, routes: Record<string, React.ReactNode>, role: Role = "admin") {
  return render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <SessionProvider value={{ username: "ana", role }}>
      <MemoryRouter initialEntries={[path]}>
        <Routes>
          <Route element={<SharedConfigDoc />}>
            {Object.entries(routes).map(([p, el]) => (
              <Route key={p} path={p} element={el} />
            ))}
          </Route>
        </Routes>
      </MemoryRouter>
      </SessionProvider>
    </QueryClientProvider>,
  );
}
