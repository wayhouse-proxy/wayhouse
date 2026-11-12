import { useCallback, useEffect, useRef, useState } from "react";
import { ApiError, fetchRegistryPlugin, getRegistryPlugin, listPluginRegistries, listRegistryPlugins } from "../../api";
import type { PluginFetchResult, RegistryPlugin, RegistryPluginCatalogEntry, RegistryPlugins, RegistryRef } from "../../types";
import { CatalogBrowser } from "../CatalogBrowser";
import { RegistryRiskDialog } from "../RegistryRiskDialog";
import { ErrorState, Loading } from "../shell/States";
import { Badge } from "../ui/Badge";
import { Button } from "../ui/Button";
import { Select } from "../ui/Input";
import { bytes } from "../../lib/plugins/capabilities";

const errorText = (err: unknown) => (err instanceof ApiError ? err.message : String(err));

const UNVERIFIED_TEXT =
  "This module's signature could not be verified, so nothing proves it is the one its author published. A plugin runs sandboxed " +
  "inside the controller and only does what you approve in the next step, but it can misuse what you approve.";
const EXTERNAL_TEXT =
  "This registry is external, at your own risk. Nobody on this project has reviewed its modules, and this one's signature could not be verified. " +
  "A plugin runs sandboxed inside the controller and only does what you approve in the next step.";

/** The newest version built for this controller's ABI (the index is sorted newest first). */
const newestCompatible = (p: RegistryPlugin) => p.versions.find((v) => v.compatible);

/** Size and the options a plugin asks for come with its detail file, fetched when the operator opens this. */
function PluginDetails({ registryId, name }: { registryId: string; name: string }) {
  const [open, setOpen] = useState(false);
  const [plugin, setPlugin] = useState<RegistryPlugin | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    if (!open || plugin) return;
    let live = true;
    getRegistryPlugin(registryId, name)
      .then((d) => live && setPlugin(d.plugin))
      .catch((err) => live && setError(errorText(err)));
    return () => {
      live = false;
    };
  }, [open, plugin, registryId, name]);
  const v = plugin && newestCompatible(plugin);
  const options = Object.entries(v?.options_schema?.properties ?? {});
  return (
    <details className="mt-1 text-sm" onToggle={(e) => setOpen(e.currentTarget.open)}>
      <summary className="cursor-pointer text-ink-muted">Details</summary>
      {error && <p className="mt-1 text-bad">{error}</p>}
      {open && !plugin && !error && <p className="mt-1 text-ink-muted">Loading…</p>}
      {plugin && !v && <p className="mt-1 text-bad">No version is built for this controller; the newest is built for plugin ABI {plugin.versions[0]?.abi}.</p>}
      {v && (
        <>
          <p className="mt-1 text-ink-muted">
            {v.version} · {bytes(v.size)} · {v.verifiable ? "signature checked on fetch" : "signature not verified"}
          </p>
          {options.length > 0 && (
            <ul className="mt-1 list-disc pl-5 text-ink-muted">
              {options.map(([key, o]) => (
                <li key={key}>
                  <span className="text-ink">{o.title ?? key}</span>
                  {o.description && ` — ${o.description}`}
                </li>
              ))}
            </ul>
          )}
        </>
      )}
    </details>
  );
}

// Fetching from a plugin registry only uploads the module to the controller; the install wizard still shows the
// options, the capabilities and asks for the approval.
export function PluginRegistrySection({ onFetched }: { onFetched: (result: PluginFetchResult, registry: RegistryRef) => void }) {
  const [registries, setRegistries] = useState<RegistryRef[] | null>(null);
  const [registryId, setRegistryId] = useState("");
  const [listing, setListing] = useState<RegistryPlugins | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  // A result that arrives after the section was dropped (Start over) must not reach the wizard.
  const alive = useRef(true);
  useEffect(() => {
    alive.current = true; // StrictMode runs the cleanup once before the real mount
    return () => {
      alive.current = false;
    };
  }, []);
  const [confirming, setConfirming] = useState<{ plugin: string; version: string | undefined; risk: "official" | "external" } | null>(null);

  useEffect(() => {
    let live = true;
    listPluginRegistries()
      .then(({ registries }) => {
        if (!live) return;
        setRegistries(registries);
        setRegistryId((id) => id || registries[0]?.id || "");
      })
      .catch((err) => live && setError(errorText(err)));
    return () => {
      live = false;
    };
  }, []);

  const load = useCallback(async (id: string) => {
    setListing(null);
    setError(null);
    try {
      setListing(await listRegistryPlugins(id));
    } catch (err) {
      setError(errorText(err));
    }
  }, []);
  useEffect(() => {
    if (registryId) load(registryId);
  }, [registryId, load]);

  async function fetchIt(plugin: string, version: string | undefined, allowUnsigned: boolean) {
    if (!listing || busy) return;
    const registry = listing.registry;
    setBusy(plugin);
    setError(null);
    try {
      const result = await fetchRegistryPlugin(registry.id, plugin, version, allowUnsigned);
      if (!alive.current) return;
      setConfirming(null);
      onFetched(result, registry);
    } catch (err) {
      if (!alive.current) return;
      if (err instanceof ApiError && err.status === 409 && err.body?.needs_confirm) {
        setConfirming({ plugin, version, risk: err.body.risk === "official" ? "official" : "external" });
      } else {
        setConfirming(null);
        setError(errorText(err));
      }
    } finally {
      if (alive.current) setBusy(null);
    }
  }

  if (!registries && !error) return <Loading />;
  if (registries && registries.length === 0) return <p className="text-sm text-ink-muted">No plugin registry is configured.</p>;

  return (
    <div className="flex flex-col gap-3">
      {registries && registries.length > 1 && (
        <label className="flex flex-col gap-1 text-sm text-ink-muted">
          Registry
          <Select value={registryId} disabled={busy !== null} onChange={(e) => setRegistryId(e.target.value)}>
            {registries.map((r) => (
              <option key={r.id} value={r.id}>
                {r.name}
                {r.official ? "" : " (external)"}
              </option>
            ))}
          </Select>
        </label>
      )}
      {error && <ErrorState onRetry={registryId ? () => load(registryId) : undefined}>{error}</ErrorState>}
      {registryId && !listing && !error && <Loading />}
      {listing && (
        <CatalogBrowser items={listing.plugins}>
          {(page) => (
            <ul className="flex flex-col gap-2">
              {page.map((p: RegistryPluginCatalogEntry) => (
                <li key={p.name} className="rounded-sm border border-line p-3">
                  <div className="flex flex-wrap items-center gap-2">
                    <span className="font-mono text-ink">{p.name}</span>
                    <span className="text-xs text-ink-muted">{p.latest}</span>
                    {p.signed ? <Badge tone="neutral">signature listed</Badge> : <Badge tone="warn">unsigned</Badge>}
                    <span className="ml-auto" />
                    <Button
                      variant="primary"
                      disabled={!p.abi_compatible || busy !== null}
                      loading={busy === p.name}
                      aria-label={`Fetch ${p.name}`}
                      onClick={() => fetchIt(p.name, undefined, false)}
                    >
                      Fetch
                    </Button>
                  </div>
                  <p className="mt-1 text-sm text-ink-muted">
                    {p.description} · {p.license}
                  </p>
                  {(p.categories.length > 0 || p.tags.length > 0) && (
                    <p className="mt-0.5 text-xs text-ink-muted">{[...p.categories, ...p.tags].join(" · ")}</p>
                  )}
                  {!p.abi_compatible && (
                    <p className="mt-1 text-sm text-bad">
                      The newest version is built for plugin ABI {p.abi}, this controller speaks {listing.host_abi}; open Details to see whether an older one fits.
                    </p>
                  )}
                  <PluginDetails registryId={listing.registry.id} name={p.name} />
                </li>
              ))}
            </ul>
          )}
        </CatalogBrowser>
      )}
      <p className="text-sm text-ink-muted">Fetching only uploads the module to the controller. You still see its options and approve what it may do before anything is installed.</p>
      {confirming && (
        <RegistryRiskDialog
          open
          onOpenChange={(o) => !o && setConfirming(null)}
          title={`Fetch ${confirming.plugin} without a verified signature?`}
          confirmLabel="Fetch anyway"
          riskText={confirming.risk === "external" ? EXTERNAL_TEXT : UNVERIFIED_TEXT}
          busy={busy !== null}
          onConfirm={() => fetchIt(confirming.plugin, confirming.version, true)}
        >
          <p className="text-ink-muted">
            {confirming.plugin} {confirming.version ?? ""} from {listing?.registry.name}.
          </p>
        </RegistryRiskDialog>
      )}
    </div>
  );
}
