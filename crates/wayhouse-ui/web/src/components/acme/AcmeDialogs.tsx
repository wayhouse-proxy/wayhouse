import { useState } from "react";
import { ApiError, createAcmeGateway, putAcmeSite, type AcmeSiteInput } from "../../api";
import type { AcmeGateway, AcmeSite } from "../../types";
import { Button } from "../ui/Button";
import { Dialog } from "../ui/Dialog";
import { Input, Select, Textarea } from "../ui/Input";
import { Switch } from "../ui/Switch";

const SITE_NAME = /^[a-z0-9-]{1,63}$/;
const lines = (text: string) => text.split("\n").map((l) => l.trim()).filter(Boolean);

/** What the controller would refuse, said before the call; it stays the authority. */
export function siteProblem(name: string, input: AcmeSiteInput): string | null {
  if (!SITE_NAME.test(name)) return "A site name is 1 to 63 of a-z, 0-9 and '-'.";
  if (input.names.length === 0) return "Give at least one name.";
  if (input.names.length > 100) return "A site has at most 100 names (the CA limit).";
  const wildcard = input.names.some((n) => n.startsWith("*."));
  if (wildcard && input.gateways.length === 0) return "A wildcard needs at least one gateway: its key should live only on a few hardened nodes.";
  if (wildcard && input.challenge === "http-01") return "HTTP-01 cannot cover a wildcard; use DNS-01.";
  if (input.challenge === "dns-01" && !input.dns_plugin) return "DNS-01 needs a DNS plugin.";
  return null;
}

/** Create a site, or replace one (its name is then fixed). The body carries only what the controller accepts. */
export function SiteDialog({ site, gateways, onClose, onSaved }: { site: AcmeSite | null; gateways: AcmeGateway[]; onClose: () => void; onSaved: (name: string) => void }) {
  const [name, setName] = useState(site?.name ?? "");
  const [names, setNames] = useState(site?.names.join("\n") ?? "");
  const [challenge, setChallenge] = useState<"dns-01" | "http-01">(site?.challenge ?? "dns-01");
  const [plugin, setPlugin] = useState(site?.dns_plugin ?? "");
  const [zone, setZone] = useState(site?.challenge_zone ?? "");
  const [picked, setPicked] = useState<string[]>(site?.gateways ?? []);
  const [enabled, setEnabled] = useState(site?.enabled ?? true);
  const [problem, setProblem] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  // A gateway the site still names but the controller no longer has stays listed: saving would send it back.
  const stale = (site?.gateways ?? []).filter((g) => !gateways.some((x) => x.name === g));

  async function save(e: React.FormEvent) {
    e.preventDefault();
    const input: AcmeSiteInput = {
      names: lines(names),
      challenge,
      gateways: picked,
      enabled,
      ...(challenge === "dns-01" ? { dns_plugin: plugin, ...(zone ? { challenge_zone: zone } : {}) } : {}),
      ...(site ? { propagation_timeout_secs: site.propagation_timeout_secs } : {}),
    };
    const bad = siteProblem(name, input);
    if (bad) return setProblem(bad);
    setProblem(null);
    setBusy(true);
    try {
      await putAcmeSite(name, input);
      onSaved(name);
    } catch (err) {
      setProblem(err instanceof ApiError ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <Dialog open onOpenChange={(o) => !o && onClose()} title={site ? `Edit site ${site.name}` : "Add ACME site"} description="The controller issues and renews one certificate for the names of a site.">
      <form onSubmit={save} className="flex flex-col gap-3 text-sm">
        <label className="flex flex-col gap-1">
          <span className="font-semibold">Site name</span>
          <Input mono value={name} disabled={!!site} onChange={(e) => setName(e.target.value)} />
        </label>
        <label className="flex flex-col gap-1">
          <span className="font-semibold">Names, one per line</span>
          <Textarea rows={3} className="font-mono" value={names} onChange={(e) => setNames(e.target.value)} placeholder={"panel.example.com\n*.example.com"} />
        </label>
        <label className="flex flex-col gap-1">
          <span className="font-semibold">Challenge</span>
          <Select aria-label="Challenge" value={challenge} onChange={(e) => setChallenge(e.target.value as "dns-01" | "http-01")}>
            <option value="dns-01">DNS-01 (a TXT record; works for wildcards)</option>
            <option value="http-01">HTTP-01 (port 80 of each name; no wildcards)</option>
          </Select>
        </label>
        {challenge === "dns-01" && (
          <>
            <label className="flex flex-col gap-1">
              <span className="font-semibold">DNS plugin</span>
              <Input mono value={plugin} onChange={(e) => setPlugin(e.target.value)} />
            </label>
            <label className="flex flex-col gap-1">
              <span className="font-semibold">Challenge zone (optional)</span>
              <Input mono value={zone} onChange={(e) => setZone(e.target.value)} />
            </label>
          </>
        )}
        <fieldset>
          <legend className="mb-1 font-semibold">Gateways that may fetch it</legend>
          {gateways.length === 0 && stale.length === 0 && <p className="text-ink-muted">No gateway is registered yet.</p>}
          {[...gateways.map((g) => g.name), ...stale].map((g) => (
            <label key={g} className="flex items-center gap-2 py-0.5">
              <input type="checkbox" checked={picked.includes(g)} onChange={(e) => setPicked(e.target.checked ? [...picked, g] : picked.filter((x) => x !== g))} />
              <span className="font-mono">{g}</span>
              {stale.includes(g) && <span className="text-ink-muted">(not registered; uncheck to drop it)</span>}
            </label>
          ))}
        </fieldset>
        <label className="flex items-center gap-2">
          <Switch checked={enabled} onCheckedChange={setEnabled} aria-label="Enabled" />
          Enabled
        </label>
        {problem && (
          <p role="alert" className="text-bad">
            {problem}
          </p>
        )}
        <div className="flex justify-end gap-2">
          <Button type="button" onClick={onClose}>
            Cancel
          </Button>
          <Button type="submit" variant="primary" loading={busy}>
            {site ? "Save site" : "Add site"}
          </Button>
        </div>
      </form>
    </Dialog>
  );
}

/** Register a gateway; the controller answers with its token once, and this is the only place it is shown. */
export function GatewayDialog({ onClose, onCreated }: { onClose: () => void; onCreated: () => void }) {
  const [name, setName] = useState("");
  const [token, setToken] = useState<string | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [saved, setSaved] = useState(false);

  async function create(e: React.FormEvent) {
    e.preventDefault();
    if (!SITE_NAME.test(name)) return setProblem("A gateway name is 1 to 63 of a-z, 0-9 and '-'.");
    setProblem(null);
    setBusy(true);
    try {
      setToken((await createAcmeGateway(name)).token);
      onCreated();
    } catch (err) {
      setProblem(err instanceof ApiError ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <Dialog open onOpenChange={(o) => !o && !token && onClose()} title="Register gateway" description="A gateway fetches the certificates of the sites it is allowed to read.">
      {token ? (
        <div className="flex flex-col gap-3 text-sm">
          <p>
            Token for <span className="font-mono">{name}</span>. Copy it now: it is not shown again.
          </p>
          <Input mono readOnly aria-label="Gateway token" value={token} onFocus={(e) => e.currentTarget.select()} />
          <label className="flex items-center gap-2">
            <input type="checkbox" checked={saved} onChange={(e) => setSaved(e.target.checked)} />I have copied the token
          </label>
          <div className="flex justify-end">
            <Button variant="primary" disabled={!saved} onClick={onClose}>
              Done
            </Button>
          </div>
        </div>
      ) : (
        <form onSubmit={create} className="flex flex-col gap-3 text-sm">
          <label className="flex flex-col gap-1">
            <span className="font-semibold">Gateway name</span>
            <Input mono value={name} onChange={(e) => setName(e.target.value)} />
          </label>
          {problem && (
            <p role="alert" className="text-bad">
              {problem}
            </p>
          )}
          <div className="flex justify-end gap-2">
            <Button type="button" onClick={onClose}>
              Cancel
            </Button>
            <Button type="submit" variant="primary" loading={busy}>
              Register
            </Button>
          </div>
        </form>
      )}
    </Dialog>
  );
}
