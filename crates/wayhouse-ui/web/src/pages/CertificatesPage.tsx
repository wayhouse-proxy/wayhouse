import { useEffect, useState } from "react";
import { ApiError, getAcmeAccount, listAcmeGateways, listAcmeSites } from "../api";
import type { AcmeAccount, AcmeGateway, AcmeSite } from "../types";
import { EmptyState, ErrorState, Loading } from "../components/shell/States";
import { PageHeader } from "../components/shell/Page";
import { Badge } from "../components/ui/Badge";
import { Button } from "../components/ui/Button";
import { Table, TBody, TD, TH, THead, TR } from "../components/ui/Table";

const DAY = 86_400;
/** Under this many days left, a certificate is "expires soon" and a failing renewal is urgent. */
const SOON = 14 * DAY;

function exact(unixSecs: number): string {
  return new Date(unixSecs * 1000).toISOString();
}

/** "in 12 d" / "3 d ago" / "in 5 h" from unix seconds; the exact time is the cell's tooltip. */
function relative(unixSecs: number): string {
  const delta = unixSecs - Math.floor(Date.now() / 1000);
  const abs = Math.abs(delta);
  const text =
    abs < 3600
      ? `${Math.max(1, Math.floor(abs / 60))} min`
      : abs < DAY
        ? `${Math.floor(abs / 3600)} h`
        : `${Math.floor(abs / DAY)} d`;
  return delta >= 0 ? `in ${text}` : `${text} ago`;
}

/** The one badge a site row shows, most urgent first. */
function status(site: AcmeSite): { tone: "good" | "warn" | "bad" | "neutral"; label: string } {
  if (!site.enabled) return { tone: "neutral", label: "disabled" };
  const cert = site.certificate;
  const now = Math.floor(Date.now() / 1000);
  if (cert && cert.not_after <= now) return { tone: "bad", label: "expired" };
  if (cert?.held) return { tone: "bad", label: "key unreadable" };
  if (site.attempt?.last_error) {
    // A failing renewal is only a warning while the certificate has time left.
    const urgent = !cert || cert.not_after - now < SOON;
    return { tone: urgent ? "bad" : "warn", label: cert ? "renewal failing" : "failing" };
  }
  if (!cert) return { tone: "warn", label: "pending" };
  if (cert.untrusted) return { tone: "warn", label: "staging (untrusted)" };
  if (cert.not_after - now < SOON) return { tone: "warn", label: "expires soon" };
  return { tone: "good", label: "valid" };
}

// Read-only view of the controller's ACME state (`GET /acme/sites`, `/acme/gateways`,
// `/acme/account`; docs/acme.md). Sites and gateways are managed through the controller's API.
export function CertificatesPage() {
  const [sites, setSites] = useState<AcmeSite[] | null>(null);
  const [gateways, setGateways] = useState<AcmeGateway[]>([]);
  const [account, setAccount] = useState<AcmeAccount | null>(null);
  const [error, setError] = useState<Error | null>(null);
  // A failed gateway or account read must not hide the sites, so each part reports on its own.
  const [partial, setPartial] = useState<string[]>([]);
  const [loading, setLoading] = useState(true);

  async function refresh() {
    setLoading(true);
    const [s, g, a] = await Promise.allSettled([listAcmeSites(), listAcmeGateways(), getAcmeAccount()]);
    const reason = (r: PromiseRejectedResult) =>
      r.reason instanceof Error ? r.reason.message : String(r.reason);
    if (s.status === "fulfilled") {
      setSites(s.value.sites);
      setError(null);
    } else {
      setSites(null);
      setError(s.reason instanceof Error ? s.reason : new Error(String(s.reason)));
    }
    const failed: string[] = [];
    if (g.status === "fulfilled") setGateways(g.value.gateways);
    else {
      setGateways([]);
      failed.push(`gateways: ${reason(g)}`);
    }
    if (a.status === "fulfilled") setAccount(a.value);
    else {
      setAccount(null);
      failed.push(`CA account: ${reason(a)}`);
    }
    setPartial(failed);
    setLoading(false);
  }

  useEffect(() => {
    refresh();
  }, []);

  return (
    <div className="max-w-5xl">
      <PageHeader
        title="Certificates"
        summary="Certificates the controller issues and renews, and the gateways that fetch them."
        actions={
          <Button onClick={refresh} loading={loading}>
            Refresh
          </Button>
        }
      />

      {error && (
        <ErrorState>
          {error.message}
          {error instanceof ApiError && error.status === 501 && (
            <>
              {" "}
              (start the controller with <span className="font-mono">--acme</span>)
            </>
          )}
        </ErrorState>
      )}

      {partial.map((msg) => (
        <ErrorState key={msg}>Could not read {msg}</ErrorState>
      ))}

      {sites === null && loading && !error && <Loading />}

      {account && (
        <p className="mb-4 text-sm text-ink-muted">
          CA <span className="font-mono text-ink">{account.directory_url}</span>
          {" · "}
          {account.account_url ? (
            <>
              account <span className="font-mono text-ink">{account.account_url}</span>
            </>
          ) : (
            "no account registered yet"
          )}
        </p>
      )}

      {sites && (
        <>
          <h2 className="mb-2 text-lg font-semibold">Sites</h2>
          <div className="mb-6">
            {sites.length === 0 ? (
              <EmptyState>No ACME sites configured.</EmptyState>
            ) : (
              <Table>
                <THead>
                  <TR>
                    <TH>site</TH>
                    <TH>names</TH>
                    <TH>challenge</TH>
                    <TH>expires</TH>
                    <TH>status</TH>
                  </TR>
                </THead>
                <TBody>
                  {sites.map((s) => {
                    const st = status(s);
                    return (
                      <TR key={s.name} className="align-top">
                        <TD className="font-mono text-ink">{s.name}</TD>
                        <TD className="font-mono text-ink-muted">{s.names.join(", ")}</TD>
                        <TD className="text-ink-muted">
                          {s.challenge}
                          {s.challenge_zone && ` via ${s.challenge_zone}`}
                        </TD>
                        <TD
                          className="text-ink-muted"
                          title={s.certificate ? exact(s.certificate.not_after) : undefined}
                        >
                          {s.certificate ? relative(s.certificate.not_after) : "–"}
                        </TD>
                        <TD>
                          <Badge tone={st.tone}>{st.label}</Badge>
                          {s.attempt?.last_error && (
                            <p className="mt-1 max-w-sm break-words text-xs text-ink-muted">
                              {s.attempt.last_error}
                              {s.attempt.failures > 0 &&
                                ` (${s.attempt.failures} failures, next try ${relative(s.attempt.next_attempt_at)})`}
                            </p>
                          )}
                          {s.certificate?.held && (
                            <p className="mt-1 text-xs text-ink-muted">{s.certificate.held}</p>
                          )}
                        </TD>
                      </TR>
                    );
                  })}
                </TBody>
              </Table>
            )}
          </div>

          <h2 className="mb-2 text-lg font-semibold">Gateways</h2>
          <div>
            {gateways.length === 0 ? (
              <EmptyState>No gateways registered.</EmptyState>
            ) : (
              <Table>
                <THead>
                  <TR>
                    <TH>gateway</TH>
                    <TH>created</TH>
                    <TH>may fetch</TH>
                  </TR>
                </THead>
                <TBody>
                  {gateways.map((g) => (
                    <TR key={g.name}>
                      <TD className="font-mono text-ink">{g.name}</TD>
                      <TD className="text-ink-muted" title={exact(g.created_at)}>
                        {relative(g.created_at)}
                      </TD>
                      <TD className="font-mono text-ink-muted">
                        {g.sites.length > 0 ? g.sites.join(", ") : "no site"}
                      </TD>
                    </TR>
                  ))}
                </TBody>
              </Table>
            )}
          </div>
        </>
      )}
    </div>
  );
}
