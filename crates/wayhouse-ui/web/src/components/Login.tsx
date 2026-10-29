import { ErrorState } from "./shell/States";
import { useState } from "react";
import { ApiError, login } from "../api";
import { Button, Input } from "./ui/Button";

export function Login({ onLoggedIn }: { onLoggedIn: () => void }) {
  const [password, setPassword] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  async function submit(e: React.FormEvent) {
    e.preventDefault();
    setBusy(true);
    setError(null);
    try {
      await login(password);
      onLoggedIn();
    } catch (err) {
      setError(err instanceof ApiError ? err.message : "login failed");
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="flex h-screen items-center justify-center bg-bg">
      <form
        className="flex w-full max-w-xs flex-col gap-4 rounded-sm border border-line bg-surface p-6"
        onSubmit={submit}
      >
        <div className="flex items-center gap-2">
          <span aria-hidden className="h-2 w-2 bg-ink" />
          <h1 className="text-lg font-semibold">wayhouse</h1>
        </div>
        <label className="flex flex-col gap-1 text-sm font-medium">
          Password
          <Input
            type="password"
            value={password}
            onChange={(e) => setPassword(e.target.value)}
            autoFocus
          />
        </label>
        {error && <ErrorState>{error}</ErrorState>}
        <Button type="submit" variant="primary" loading={busy}>
          Sign in
        </Button>
      </form>
    </div>
  );
}
