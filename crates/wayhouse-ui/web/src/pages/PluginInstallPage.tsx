import { useRef, useState } from "react";
import { Link, useNavigate } from "react-router-dom";
import { ApiError, installPlugin, resolvePluginCapabilities, uploadPluginModule } from "../api";
import type { PluginCapabilities, PluginModule } from "../types";
import { ApprovalDialog } from "../components/plugins/ApprovalDialog";
import { SchemaForm } from "../components/form/SchemaForm";
import { PageHeader } from "../components/shell/Page";
import { ErrorState, Forbidden } from "../components/shell/States";
import { Button, Input } from "../components/ui/Button";
import { Switch } from "../components/ui/Switch";
import { useCan } from "../lib/session";
import { bytes } from "../lib/plugins/capabilities";
import { optionsForm } from "../lib/plugins/options";

const MAX_MODULE_BYTES = 8 * 1024 * 1024;

const errorText = (err: unknown) => (err instanceof ApiError ? err.message : String(err));

/** A file name as a plugin name: lowercase a-z, 0-9 and '-', at most 64 characters. */
export function suggestName(fileName: string): string {
  return fileName
    .replace(/\.wasm$/i, "")
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 64);
}

type Step = "upload" | "options" | "review";

const STEPS: [Step, string][] = [
  ["upload", "1. Module"],
  ["options", "2. Options"],
  ["review", "3. Review and install"],
];

// The install wizard (docs/plugins.md): upload and inspect, fill in the module's options, see what
// the options resolve to, approve, name and enable. Nothing is installed before the approval.
export function PluginInstallPage() {
  const canWrite = useCan("admin");
  const navigate = useNavigate();
  const [step, setStep] = useState<Step>("upload");
  const [file, setFile] = useState<File | null>(null);
  const [module, setModule] = useState<PluginModule | null>(null);
  const [config, setConfig] = useState<Record<string, unknown>>({});
  const [effective, setEffective] = useState<PluginCapabilities | null>(null);
  const [name, setName] = useState("");
  const [enabled, setEnabled] = useState(true);
  const [asking, setAsking] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // Bumped on Start over so a slow answer for a dropped module cannot land later.
  const session = useRef(0);

  if (!canWrite) return <Forbidden />;

  const form = optionsForm(module?.manifest);

  function reset() {
    session.current += 1;
    setStep("upload");
    setFile(null);
    setModule(null);
    setConfig({});
    setEffective(null);
    setAsking(false);
    setBusy(false);
    setError(null);
  }

  const request = (approved: PluginCapabilities) => ({
    name,
    sha256: module!.sha256,
    approved,
    config,
    enabled,
  });

  /** Asks the controller what the options resolve to, then shows the review. */
  async function review(m: PluginModule, cfg: Record<string, unknown>) {
    const mine = session.current;
    setBusy(true);
    setError(null);
    try {
      const { effective } = await resolvePluginCapabilities({
        name: name || suggestName(file?.name ?? "plugin") || "plugin",
        sha256: m.sha256,
        approved: m.capabilities,
        config: cfg,
        enabled,
      });
      if (mine !== session.current) return;
      setEffective(effective);
      setStep("review");
    } catch (err) {
      if (mine === session.current) setError(errorText(err));
    } finally {
      if (mine === session.current) setBusy(false);
    }
  }

  async function inspect(e: React.FormEvent) {
    e.preventDefault();
    if (!file) return setError("choose a .wasm file");
    if (file.size > MAX_MODULE_BYTES) return setError(`the file is ${bytes(file.size)}; modules are limited to 8 MiB`);
    const mine = session.current;
    setBusy(true);
    setError(null);
    try {
      const inspected = await uploadPluginModule(await file.arrayBuffer());
      if (mine !== session.current) return;
      setModule(inspected);
      setName(suggestName(file.name));
      if (optionsForm(inspected.manifest).hasFields) {
        setStep("options");
        setBusy(false);
      } else {
        await review(inspected, {});
      }
    } catch (err) {
      if (mine === session.current) setError(errorText(err));
    } finally {
      if (mine === session.current) setBusy(false);
    }
  }

  async function install() {
    if (!module || !effective) return;
    const mine = session.current;
    setBusy(true);
    setError(null);
    try {
      // Exactly the set that was shown: approval is bound to what the options resolve to.
      await installPlugin(request(effective));
      navigate("/plugins", { state: { notice: `installed ${name}` } });
    } catch (err) {
      if (mine !== session.current) return;
      // The set moved between the review and the install (an HA node on another build): show the new one.
      if (err instanceof ApiError && err.status === 400 && err.body?.effective) {
        setEffective(err.body.effective);
        setError("What the options resolve to changed; check the new list and approve again.");
      } else setError(errorText(err));
    } finally {
      if (mine === session.current) setBusy(false);
    }
  }

  return (
    <div className="max-w-3xl">
      <PageHeader
        title="Install a plugin"
        summary="Upload a module, set its options, then approve what it may do. Nothing runs before you approve."
        actions={
          <Link to="/plugins" className="text-sm text-ink-muted underline">
            Back to plugins
          </Link>
        }
      />
      <ol className="mb-4 flex gap-4 text-sm text-ink-muted" aria-label="Steps">
        {STEPS.map(([s, label]) => (
          <li key={s} aria-current={s === step ? "step" : undefined} className={s === step ? "font-semibold text-ink" : undefined}>
            {label}
          </li>
        ))}
      </ol>

      {step === "upload" && (
        <form className="flex flex-col gap-3" onSubmit={inspect}>
          <label className="flex flex-col gap-1 text-sm text-ink-muted">
            .wasm file
            <input type="file" accept=".wasm" onChange={(e) => setFile(e.target.files?.[0] ?? null)} className="text-sm text-ink" />
          </label>
          {error && <ErrorState>{error}</ErrorState>}
          <div>
            <Button type="submit" variant="primary" loading={busy}>
              {busy ? "Uploading…" : "Inspect module"}
            </Button>
          </div>
        </form>
      )}

      {module && (
        <p className="mb-3 text-sm text-ink-muted">
          Module <span className="font-mono text-xs text-ink">{module.sha256.slice(0, 12)}…</span>, {bytes(module.size)}, plugin ABI{" "}
          {module.abi}. Uploaded by hand: no registry vouches for it.{" "}
          <button type="button" className="underline" onClick={reset}>
            Start over
          </button>
        </p>
      )}

      {step === "options" && module && (
        <div className="flex flex-col gap-3">
          <SchemaForm schema={form.schema} layout={form.layout} value={config} onChange={(v) => setConfig(v as Record<string, unknown>)} />
          {form.secrets.length > 0 && (
            <p className="text-sm text-ink-muted">
              {form.secrets.join(", ")} {form.secrets.length === 1 ? "is a secret and is" : "are secrets and are"} set under Secrets
              after the install; the form never takes them.
            </p>
          )}
          {error && <ErrorState>{error}</ErrorState>}
          <div>
            <Button variant="primary" loading={busy} onClick={() => review(module, config)}>
              Review what it asks for
            </Button>
          </div>
        </div>
      )}

      {step === "review" && module && effective && (
        <div className="flex flex-col gap-3">
          <label className="flex flex-col gap-1 text-sm text-ink-muted">
            Name
            <Input value={name} onChange={(e) => setName(e.target.value)} />
          </label>
          <label className="flex items-center gap-2 text-sm text-ink-muted">
            <Switch checked={enabled} onCheckedChange={setEnabled} aria-label="Enable after installing" />
            Enable after installing
          </label>
          {step === "review" && form.hasFields && (
            <Button className="self-start" variant="ghost" onClick={() => setStep("options")}>
              Back to options
            </Button>
          )}
          {error && !asking && <ErrorState>{error}</ErrorState>}
          <div>
            <Button variant="primary" disabled={name === ""} onClick={() => setAsking(true)}>
              Install…
            </Button>
          </div>
          <ApprovalDialog
            open={asking}
            title={`Approve ${name}`}
            intro="The plugin runs only with what is listed here. It can still misuse what you approve, so approve only modules you trust."
            caps={effective}
            confirmLabel="Approve and install"
            busy={busy}
            error={error}
            onConfirm={install}
            onCancel={() => setAsking(false)}
          />
        </div>
      )}
    </div>
  );
}
