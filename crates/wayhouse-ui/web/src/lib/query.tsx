import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { useState } from "react";

/** One client for the app. Data is operational, so a stale read is retried once and refetched on focus. */
export function QueryProvider({ children }: { children: React.ReactNode }) {
  const [client] = useState(() => new QueryClient({ defaultOptions: { queries: { retry: 1, staleTime: 5000 } } }));
  return <QueryClientProvider client={client}>{children}</QueryClientProvider>;
}
