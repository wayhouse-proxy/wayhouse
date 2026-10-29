import { render } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { SharedConfigDoc } from "../lib/config/SharedConfigDoc";

/** A page that edits the configuration, inside the layout route that shares its document. */
export function renderConfigPage(path: string, routes: Record<string, React.ReactNode>) {
  return render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <MemoryRouter initialEntries={[path]}>
        <Routes>
          <Route element={<SharedConfigDoc />}>
            {Object.entries(routes).map(([p, el]) => (
              <Route key={p} path={p} element={el} />
            ))}
          </Route>
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
}
