import { useEffect, useState } from "react";
import { ApiError, getAcmeAccount, listAcmeGateways, listAcmeSites } from "../api";
import type { AcmeAccount, AcmeGateway, AcmeSite } from "../types";
import { Badge } from "../components/ui/Badge";
import { Button } from "../components/ui/Button";

const DAY = 86_400;

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
    if (site.attempt?.last_error) {
        return { tone: cert ? "warn" : "bad", label: cert ? "renewal failing" : "failing" };
    }
    if (!cert) return { tone: "warn", label: "pending" };
    if (cert.held) return { tone: "bad", label: "key unreadable" };
    if (cert.untrusted) return { tone: "warn", label: "staging (untrusted)" };
    if (cert.not_after - now < 14 * DAY) return { tone: "warn", label: "expires soon" };
    return { tone: "good", label: "valid" };
}

// Read-only view of the controller's ACME state (`GET /acme/sites`, `/acme/gateways`,
// `/acme/account`; docs/acme.md). Sites and gateways are managed through the controller's API.
export function CertificatesPage() {
    const [sites, setSites] = useState<AcmeSite[] | null>(null);
    const [gateways, setGateways] = useState<AcmeGateway[]>([]);
    const [account, setAccount] = useState<AcmeAccount | null>(null);
    const [error, setError] = useState<string | null>(null);
    const [loading, setLoading] = useState(true);

    async function refresh() {
        setLoading(true);
        try {
            const [s, g, a] = await Promise.all([
                listAcmeSites(),
                listAcmeGateways(),
                getAcmeAccount(),
            ]);
            setSites(s.sites);
            setGateways(g.gateways);
            setAccount(a);
            setError(null);
        } catch (err) {
            setSites(null);
            setError(err instanceof ApiError ? err.message : String(err));
        } finally {
            setLoading(false);
        }
    }

    useEffect(() => {
        refresh();
    }, []);

    return (
        <div className="max-w-5xl">
            <div className="mb-5 flex items-center justify-between">
                <h1 className="text-lg font-semibold text-ink">Certificates</h1>
                <Button onClick={refresh} disabled={loading}>
                    Refresh
                </Button>
            </div>

            {error && (
                <p className="mb-4 rounded border border-line bg-surface px-3 py-2 text-sm text-ink-muted">
                    {error} (is the controller running with{" "}
                    <span className="font-mono">--acme</span>?)
                </p>
            )}

            {sites === null && loading && !error && (
                <p className="text-sm text-ink-muted">Loading…</p>
            )}

            {account && (
                <p className="mb-4 text-sm text-ink-muted">
                    CA <span className="font-mono text-ink">{account.directory_url}</span>
                    {" · "}
                    {account.account_url ? (
                        <>
                            account{" "}
                            <span className="font-mono text-ink">{account.account_url}</span>
                        </>
                    ) : (
                        "no account registered yet"
                    )}
                </p>
            )}

            {sites && (
                <>
                    <h2 className="mb-2 text-sm font-semibold text-ink">Sites</h2>
                    <div className="mb-6 rounded border border-line bg-surface">
                        {sites.length === 0 ? (
                            <p className="px-4 py-3 text-sm text-ink-muted">
                                No ACME sites configured.
                            </p>
                        ) : (
                            <table className="w-full text-sm">
                                <thead className="text-ink-faint">
                                    <tr className="text-left">
                                        <th className="px-4 py-2 font-medium">site</th>
                                        <th className="px-4 py-2 font-medium">names</th>
                                        <th className="px-4 py-2 font-medium">challenge</th>
                                        <th className="px-4 py-2 font-medium">expires</th>
                                        <th className="px-4 py-2 font-medium">status</th>
                                    </tr>
                                </thead>
                                <tbody>
                                    {sites.map((s) => {
                                        const st = status(s);
                                        return (
                                            <tr
                                                key={s.name}
                                                className="border-t border-line align-top"
                                            >
                                                <td className="px-4 py-2 font-mono text-ink">
                                                    {s.name}
                                                </td>
                                                <td className="px-4 py-2 font-mono text-ink-muted">
                                                    {s.names.join(", ")}
                                                </td>
                                                <td className="px-4 py-2 text-ink-muted">
                                                    {s.challenge}
                                                    {s.challenge_zone && ` via ${s.challenge_zone}`}
                                                </td>
                                                <td
                                                    className="px-4 py-2 text-ink-muted"
                                                    title={
                                                        s.certificate
                                                            ? exact(s.certificate.not_after)
                                                            : undefined
                                                    }
                                                >
                                                    {s.certificate
                                                        ? relative(s.certificate.not_after)
                                                        : "–"}
                                                </td>
                                                <td className="px-4 py-2">
                                                    <Badge tone={st.tone}>{st.label}</Badge>
                                                    {s.attempt?.last_error && (
                                                        <p className="mt-1 max-w-sm break-words text-xs text-ink-muted">
                                                            {s.attempt.last_error}
                                                            {s.attempt.failures > 0 &&
                                                                ` (${s.attempt.failures} failures, next try ${relative(s.attempt.next_attempt_at)})`}
                                                        </p>
                                                    )}
                                                    {s.certificate?.held && (
                                                        <p className="mt-1 text-xs text-ink-muted">
                                                            {s.certificate.held}
                                                        </p>
                                                    )}
                                                </td>
                                            </tr>
                                        );
                                    })}
                                </tbody>
                            </table>
                        )}
                    </div>

                    <h2 className="mb-2 text-sm font-semibold text-ink">Gateways</h2>
                    <div className="rounded border border-line bg-surface">
                        {gateways.length === 0 ? (
                            <p className="px-4 py-3 text-sm text-ink-muted">
                                No gateways registered.
                            </p>
                        ) : (
                            <table className="w-full text-sm">
                                <thead className="text-ink-faint">
                                    <tr className="text-left">
                                        <th className="px-4 py-2 font-medium">gateway</th>
                                        <th className="px-4 py-2 font-medium">created</th>
                                        <th className="px-4 py-2 font-medium">may fetch</th>
                                    </tr>
                                </thead>
                                <tbody>
                                    {gateways.map((g) => (
                                        <tr key={g.name} className="border-t border-line">
                                            <td className="px-4 py-2 font-mono text-ink">
                                                {g.name}
                                            </td>
                                            <td
                                                className="px-4 py-2 text-ink-muted"
                                                title={exact(g.created_at)}
                                            >
                                                {relative(g.created_at)}
                                            </td>
                                            <td className="px-4 py-2 font-mono text-ink-muted">
                                                {g.sites.length > 0
                                                    ? g.sites.join(", ")
                                                    : "no site"}
                                            </td>
                                        </tr>
                                    ))}
                                </tbody>
                            </table>
                        )}
                    </div>
                </>
            )}
        </div>
    );
}
