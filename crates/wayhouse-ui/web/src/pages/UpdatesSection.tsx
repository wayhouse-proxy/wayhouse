import { useRef, useState } from "react";
import { ApiError, checkUpdates, listRegistrySniffers } from "../api";
import type { RegistrySniffer, RegistryRef, UpdateCheck, UpdateCheckRow } from "../types";
import { AlertBar } from "../components/shell/Page";
import { EmptyState } from "../components/shell/States";
import { Badge } from "../components/ui/Badge";
import { Button } from "../components/ui/Button";
import { Table, TBody, TD, TH, THead, TR } from "../components/ui/Table";
import { InstallDialog } from "./RegistrySection";

function message(err: unknown): string {
  return err instanceof ApiError || err instanceof Error ? err.message : String(err);
}

interface Pending {
  registry: RegistryRef;
  sniffer: RegistrySniffer;
  version: string;
}

/**
 * Checks for newer sniffer versions when the operator asks, never on a timer.
 * The update itself is the registry install flow for one exact version, with
 * the same limits and risk dialog.
 */
export function UpdatesSection({ onChanged }: { onChanged: (notice: string) => void }) {
  const [result, setResult] = useState<UpdateCheck | null>(null);
  const [checking, setChecking] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [pending, setPending] = useState<Pending | null>(null);
  const [opening, setOpening] = useState(false);
  // Which check may still publish its answer: an install or a newer check supersedes the rest.
  const latestCheck = useRef(0);

  async function check() {
    const mine = ++latestCheck.current;
    setChecking(true);
    setError(null);
    setResult(null);
    try {
      const res = await checkUpdates();
      if (mine === latestCheck.current) setResult(res);
    } catch (err) {
      if (mine === latestCheck.current) setError(message(err));
    } finally {
      if (mine === latestCheck.current) setChecking(false);
    }
  }

  async function startUpdate(row: UpdateCheckRow) {
    const update = row.update;
    if (!update?.compatible || opening) return;
    setOpening(true);
    setError(null);
    try {
      const listing = await listRegistrySniffers(update.registry_id);
      const sniffer = listing.sniffers.find((s) => s.name === row.sniffer);
      if (!sniffer) {
        setError(`${row.sniffer} is no longer listed in ${listing.registry.name}`);
        return;
      }
      setPending({ registry: listing.registry, sniffer, version: update.version });
    } catch (err) {
      setError(message(err));
    } finally {
      setOpening(false);
    }
  }

  return (
    <section className="mt-8" aria-labelledby="updates-heading">
      <div className="mb-3 flex items-center justify-between">
        <h2 id="updates-heading" className="text-lg font-semibold">
          Updates
        </h2>
        <Button onClick={check} disabled={checking}>
          {checking ? "Checking…" : "Check for updates"}
        </Button>
      </div>
      {error && <p className="mb-3 text-sm text-bad">{error}</p>}
      {!result && !error && (
        <p className="text-sm text-ink-muted">
          Compares what the instances run with the registries, once, when you ask.
        </p>
      )}
      {result && <Results result={result} onUpdate={startUpdate} busy={opening} />}
      {pending && (
        <InstallDialog
          registry={pending.registry}
          sniffer={pending.sniffer}
          version={pending.version}
          verb="Update"
          onClose={() => setPending(null)}
          onInstalled={(name) => {
            // The list was true before the update; do not offer it again, and drop a check still in flight.
            latestCheck.current++;
            setChecking(false);
            setResult(null);
            onChanged(`updated ${name}`);
          }}
        />
      )}
    </section>
  );
}

function Results({
  result,
  onUpdate,
  busy,
}: {
  result: UpdateCheck;
  onUpdate: (row: UpdateCheckRow) => void;
  busy: boolean;
}) {
  const down = result.registries.filter((r) => !r.ok);
  return (
    <div className="flex flex-col gap-2">
      {down.map((r) => (
        <AlertBar key={r.id}>
          Could not reach registry {r.name}: {r.error}
        </AlertBar>
      ))}
      {result.instance_errors.map((e) => (
        <AlertBar key={e.instance}>
          {e.instance} did not answer (status {e.status}); it is missing from this list.
        </AlertBar>
      ))}
      <div>
        {result.sniffers.length === 0 ? (
          <EmptyState>No sniffers are installed on the instances that answered.</EmptyState>
        ) : (
          <Table>
            <THead>
              <TR>
                <TH>name</TH>
                <TH>installed</TH>
                <TH>instances</TH>
                <TH>update</TH>
              </TR>
            </THead>
            <TBody>
              {result.sniffers.map((row) => (
                <TR key={`${row.sniffer}:${row.installed_sha256}`} className="align-top">
                  <TD className="font-mono text-ink">{row.sniffer}</TD>
                  <TD className="text-ink-muted">
                    {row.known ? row.installed_version : <span title={row.installed_sha256}>unknown build</span>}
                  </TD>
                  <TD className="text-ink-muted">{row.instances.join(", ")}</TD>
                  <TD>
                    <UpdateCell row={row} onUpdate={onUpdate} busy={busy} registryDown={down.length > 0} />
                  </TD>
                </TR>
              ))}
            </TBody>
          </Table>
        )}
      </div>
      {!result.min_proxy_checked && (
        <p className="text-xs text-ink-muted">
          Compatibility is checked against sniffer ABI {result.host_abi}. Each sniffer&apos;s minimum proxy version
          could not be checked against every proxy: the aggregator is unreachable, no proxy is registered, or one
          runs a build that reports no version.
        </p>
      )}
    </div>
  );
}

function UpdateCell({
  row,
  onUpdate,
  busy,
  registryDown,
}: {
  row: UpdateCheckRow;
  onUpdate: (row: UpdateCheckRow) => void;
  busy: boolean;
  registryDown: boolean;
}) {
  if (!row.known) {
    return <span className="text-ink-muted">not in any registry, so no update is offered</span>;
  }
  if (!row.update) {
    // An unreachable registry might hold a newer version, so "up to date" would be a guess.
    return registryDown ? (
      <span className="text-ink-muted">no update found in reachable registries</span>
    ) : (
      <Badge tone="good">up to date</Badge>
    );
  }
  if (!row.update.compatible) {
    return (
      <span className="text-warn">
        {row.update.version} is out: {row.update.reason}
      </span>
    );
  }
  return (
    <Button disabled={busy} onClick={() => onUpdate(row)}>
      Update to {row.update.version}
    </Button>
  );
}
