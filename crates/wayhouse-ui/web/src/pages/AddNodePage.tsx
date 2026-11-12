import { useCallback, useEffect, useMemo, useState } from "react";
import { useSearchParams } from "react-router-dom";
import { ApiError, listEnrollments, mintEnrollment, revokeEnrollment } from "../api";
import type { EnrollmentCode, MintedEnrollment } from "../types";
import { PageHeader, AlertBar } from "../components/shell/Page";
import { ErrorState, Loading } from "../components/shell/States";
import { Button } from "../components/ui/Button";
import { FieldRow } from "../components/ui/FieldRow";
import { Input, Select } from "../components/ui/Input";
import { useConfirm } from "../components/ui/ConfirmDialog";
import { StepBlock } from "../components/enroll/StepBlock";
import { PendingCodes, relative } from "../components/enroll/PendingCodes";
import { useCan } from "../lib/session";
import {
  ROLES,
  TARGETS,
  generate,
  needsStepUp,
  unsupported,
  validBackends,
  validName,
  validNamespace,
  validPin,
  validTag,
  validUrl,
  type EnrollRole,
  type Target,
} from "../lib/enroll/commands";

const TTLS: { secs: number; label: string; stepUpOk: boolean }[] = [
  { secs: 900, label: "15 minutes", stepUpOk: true },
  { secs: 3600, label: "1 hour", stepUpOk: true },
  { secs: 86_400, label: "24 hours", stepUpOk: true },
  { secs: 604_800, label: "7 days", stepUpOk: false },
];

// Public values worth remembering between visits. The code and the step-up secret are never here.
const PREF_KEY = "wayhouse.addnode.prefs";
interface Prefs {
  controllerUrl: string;
  pin: string;
  imageTag: string;
  namespace: string;
}
function readPrefs(): Prefs {
  const fallback: Prefs = { controllerUrl: "", pin: "", imageTag: "latest", namespace: "wayhouse" };
  try {
    const raw = JSON.parse(localStorage.getItem(PREF_KEY) ?? "{}");
    return Object.fromEntries(
      (Object.keys(fallback) as (keyof Prefs)[]).map((k) => [k, typeof raw?.[k] === "string" ? raw[k] : fallback[k]]),
    ) as unknown as Prefs;
  } catch {
    return fallback;
  }
}
function writePrefs(p: Prefs) {
  try {
    localStorage.setItem(PREF_KEY, JSON.stringify(p));
  } catch {
    // storage blocked: the form just starts empty next time
  }
}

/** What the operator minted, kept in this component's memory only. */
interface Minted {
  code: MintedEnrollment;
  prompt: boolean;
  target: Target;
  backends: string;
  aggregatorUrl: string;
  parentUrl: string;
  controllerUrl: string;
  pin: string;
  imageTag: string;
  namespace: string;
}

const isRole = (s: string | null): s is EnrollRole => ROLES.some((r) => r.role === s);

// Nodes > Add node: one code, one command per target. The code is shown once, from memory
// (never localStorage, never a URL), and the step-up secret is typed for each mint and dropped
// the moment the request returns.
export function AddNodePage() {
  const canWrite = useCan("admin");
  const [params] = useSearchParams();
  const startRole = params.get("role");

  const [role, setRole] = useState<EnrollRole>(isRole(startRole) ? startRole : "agent");
  const [target, setTarget] = useState<Target>("systemd");
  const [name, setName] = useState(params.get("name") ?? "");
  const [prefs, setPrefs] = useState<Prefs>(readPrefs);
  const [backends, setBackends] = useState(":25565");
  const [aggregatorUrl, setAggregatorUrl] = useState("");
  const [parentUrl, setParentUrl] = useState("");
  const [ttl, setTtl] = useState(900);
  const [uses, setUses] = useState(1);
  const [prompt, setPrompt] = useState(false);
  const [stepUp, setStepUp] = useState("");
  const [stepUpError, setStepUpError] = useState<string | null>(null);
  const [formError, setFormError] = useState<string | null>(null);
  const [minting, setMinting] = useState(false);
  const [minted, setMinted] = useState<Minted | null>(null);

  const [codes, setCodes] = useState<EnrollmentCode[] | null>(null);
  const [listError, setListError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const { confirm, dialog } = useConfirm();

  const refresh = useCallback(async () => {
    try {
      setCodes((await listEnrollments()).items);
      setListError(null);
    } catch (err) {
      setCodes(null);
      setListError(err instanceof ApiError ? err.message : String(err));
    }
  }, []);
  useEffect(() => {
    if (canWrite) void refresh();
  }, [canWrite, refresh]);

  const stepUpRole = needsStepUp(role);
  const blocked = unsupported(role, target);
  const maxTtl = stepUpRole ? 86_400 : 604_800;
  const ttlChoices = TTLS.filter((t) => !stepUpRole || t.stepUpOk);
  const effectiveTtl = Math.min(ttl, maxTtl);
  const usesFixed = role === "ui";
  const effectiveUses = usesFixed ? 1 : uses;
  const containerTarget = target !== "systemd";

  const errors = useMemo(() => {
    const e: Record<string, string> = {};
    if (!validName(name)) e.name = "1 to 64 letters, digits, '.', '_' or '-'.";
    if (!validUrl(prefs.controllerUrl, { httpsOnly: true })) e.controllerUrl = "An https:// URL the new node can reach.";
    if (!validPin(prefs.pin)) e.pin = "sha256: followed by 64 hex digits, as `wayhouse-controller init` printed it.";
    if (role === "agent" && !validBackends(backends)) e.backends = "host:port or :port, comma separated.";
    if ((role === "proxy" || role === "ui") && aggregatorUrl && !validUrl(aggregatorUrl)) e.aggregatorUrl = "An http(s) URL.";
    if (role === "aggregator" && parentUrl && !validUrl(parentUrl)) e.parentUrl = "An http(s) URL without credentials.";
    if (containerTarget && !validTag(prefs.imageTag)) e.imageTag = "A valid image tag.";
    if (target === "k8s" && !validNamespace(prefs.namespace)) e.namespace = "A Kubernetes namespace name.";
    if (stepUpRole && stepUp.length === 0) e.stepUp = "Required for this role.";
    if (!Number.isInteger(effectiveUses) || effectiveUses < 1 || effectiveUses > 1000) e.uses = "1 to 1000.";
    return e;
  }, [name, prefs, role, backends, aggregatorUrl, parentUrl, containerTarget, target, stepUpRole, stepUp, effectiveUses]);
  const valid = Object.keys(errors).length === 0 && !blocked;

  // Errors show once the field has been touched or a mint was tried.
  const [tried, setTried] = useState(false);
  const shown = (k: string) => (tried ? errors[k] : undefined);

  async function submit(ev: React.FormEvent) {
    ev.preventDefault();
    setTried(true);
    if (!valid || minting || !canWrite) return;
    setMinting(true);
    setFormError(null);
    setStepUpError(null);
    const secret = stepUpRole ? stepUp : undefined;
    // The secret leaves state before the request: whatever the outcome, it is not kept.
    setStepUp("");
    try {
      const code = await mintEnrollment({
        role,
        name,
        ttl_secs: effectiveTtl,
        uses: effectiveUses,
        ...(role === "aggregator" && parentUrl ? { parent_url: parentUrl } : {}),
        ...(secret ? { step_up: secret } : {}),
      });
      writePrefs(prefs);
      setMinted({
        code,
        prompt,
        target,
        backends,
        aggregatorUrl,
        parentUrl,
        controllerUrl: prefs.controllerUrl,
        pin: prefs.pin,
        imageTag: prefs.imageTag,
        namespace: prefs.namespace,
      });
      setTried(false);
      void refresh();
    } catch (err) {
      if (err instanceof ApiError && err.status === 403) {
        setStepUpError(err.message);
      } else {
        setFormError(err instanceof ApiError ? err.message : String(err));
      }
    } finally {
      setMinting(false);
    }
  }

  async function handleRevoke(c: EnrollmentCode) {
    const ok = await confirm({
      title: `Revoke the code for ${c.origin}?`,
      description:
        c.status === "pending"
          ? "The code stops working at once. A node that already used it keeps what it was given."
          : "Removes the record. A node that used it keeps what it was given.",
      confirmLabel: "Revoke code",
    });
    if (!ok) return;
    setBusy(true);
    try {
      await revokeEnrollment(c.id);
      setNotice(`revoked the code for ${c.origin}`);
    } catch (err) {
      setNotice(`revoke failed: ${err instanceof ApiError ? err.message : err}`);
    } finally {
      setBusy(false);
    }
    await refresh();
  }

  const steps = useMemo(() => {
    if (!minted) return [];
    return generate({
      role: minted.code.role,
      target: minted.target,
      name: minted.code.name,
      controllerUrl: minted.controllerUrl,
      pin: minted.pin,
      code: minted.prompt ? null : minted.code.code,
      imageTag: minted.imageTag,
      backends: minted.backends,
      aggregatorUrl: minted.aggregatorUrl,
      parentUrl: minted.parentUrl,
      namespace: minted.namespace,
    });
  }, [minted]);

  if (!canWrite) {
    return (
      <div className="max-w-4xl">
        <PageHeader title="Add node" />
        <ErrorState>Adding a node needs the admin role.</ErrorState>
      </div>
    );
  }

  return (
    <div className="max-w-4xl">
      <PageHeader
        title="Add node"
        summary="Create a one-time code and get the command that sets the new node up. The code works once and expires; nothing long-lived goes on a command line."
      />

      {minted ? (
        <div>
          <AlertBar>
            This code is shown once, here, and works {minted.code.uses === 1 ? "one time" : `${minted.code.uses} times`} until{" "}
            {new Date(minted.code.expires_at * 1000).toLocaleString()} ({relative(minted.code.expires_at)}). Leaving this page
            hides it for good; a lost code is revoked below and replaced.
          </AlertBar>
          {minted.prompt && (
            <section className="mb-4">
              <h2 className="mb-1 text-sm font-semibold">Code to paste at the prompt</h2>
              <Input mono readOnly aria-label="Enrollment code" value={minted.code.code} onFocus={(e) => e.currentTarget.select()} className="w-full max-w-xl" />
            </section>
          )}
          {steps.map((s, i) => (
            <StepBlock key={`${minted.code.id}-${i}`} step={s} index={i} />
          ))}
          <p className="mb-4 max-w-(--measure) text-xs text-ink-muted">
            The node finishes in seconds. Watch its row in the list below turn <strong>used</strong>.
          </p>
          <Button variant="primary" onClick={() => setMinted(null)}>
            Done, hide the code
          </Button>
        </div>
      ) : (
        <form onSubmit={submit} noValidate aria-label="New node" autoComplete="off">
          <FieldRow
            label="Role"
            help={ROLES.find((r) => r.role === role)?.what}
            render={(a) => (
              <Select {...a} value={role} onChange={(e) => setRole(e.target.value as EnrollRole)}>
                {ROLES.map((r) => (
                  <option key={r.role} value={r.role}>
                    {r.label}
                  </option>
                ))}
                <option disabled value="controller">
                  Controller replica (not available yet)
                </option>
              </Select>
            )}
          />
          {/* TODO(slice 4): offer a "Controller replica" role once the HA join code API
              (`--ha-join` enrollment) is on main; the disabled option above is its placeholder. */}
          <FieldRow
            label="Name"
            required
            help="What this node is called: an origin's tunnel name, a proxy's instance name."
            error={shown("name")}
            render={(a) => <Input {...a} value={name} onChange={(e) => setName(e.target.value)} mono placeholder="home-nas" />}
          />
          <FieldRow
            label="Set up with"
            help={blocked ?? undefined}
            render={(a) => (
              <Select {...a} value={target} onChange={(e) => setTarget(e.target.value as Target)}>
                {TARGETS.map((t) => (
                  <option key={t.target} value={t.target}>
                    {t.label}
                  </option>
                ))}
              </Select>
            )}
          />
          <FieldRow
            label="Controller URL"
            required
            help="Where the new node reaches the controller."
            error={shown("controllerUrl")}
            render={(a) => (
              <Input {...a} value={prefs.controllerUrl} onChange={(e) => setPrefs({ ...prefs, controllerUrl: e.target.value.trim() })} mono placeholder="https://controller.example:9901" className="w-full max-w-xl" />
            )}
          />
          <FieldRow
            label="Certificate fingerprint"
            required
            help="The node checks the controller's certificate against this before it sends the code. `wayhouse-controller init` printed it."
            error={shown("pin")}
            render={(a) => (
              <Input {...a} value={prefs.pin} onChange={(e) => setPrefs({ ...prefs, pin: e.target.value.trim() })} mono placeholder="sha256:…" className="w-full max-w-xl" />
            )}
          />
          {role === "agent" && (
            <FieldRow
              label="Game server addresses"
              required
              help="What this origin fronts, as host:port or :port, comma separated."
              error={shown("backends")}
              render={(a) => <Input {...a} value={backends} onChange={(e) => setBackends(e.target.value)} mono />}
            />
          )}
          {(role === "proxy" || role === "ui") && (
            <FieldRow
              label="Aggregator URL"
              help={role === "proxy" ? "Where this proxy reports its state. Its token comes with the code." : "The aggregator this UI reads from. Its token is not part of enrollment."}
              error={shown("aggregatorUrl")}
              render={(a) => <Input {...a} value={aggregatorUrl} onChange={(e) => setAggregatorUrl(e.target.value.trim())} mono placeholder="https://aggregator.example:9902" className="w-full max-w-xl" />}
            />
          )}
          {role === "aggregator" && (
            <FieldRow
              label="Parent aggregator URL"
              help="Only for a tier below another aggregator."
              error={shown("parentUrl")}
              render={(a) => <Input {...a} value={parentUrl} onChange={(e) => setParentUrl(e.target.value.trim())} mono className="w-full max-w-xl" />}
            />
          )}
          {containerTarget && (
            <FieldRow
              label="Image tag"
              help="The version to pull; match the controller's."
              error={shown("imageTag")}
              render={(a) => <Input {...a} value={prefs.imageTag} onChange={(e) => setPrefs({ ...prefs, imageTag: e.target.value.trim() })} mono />}
            />
          )}
          {target === "k8s" && (
            <FieldRow
              label="Namespace"
              error={shown("namespace")}
              render={(a) => <Input {...a} value={prefs.namespace} onChange={(e) => setPrefs({ ...prefs, namespace: e.target.value.trim() })} mono />}
            />
          )}
          <FieldRow
            label="Valid for"
            help={stepUpRole ? "A code that hands out the keyring or the admin token lives at most 24 hours." : undefined}
            render={(a) => (
              <Select {...a} value={effectiveTtl} onChange={(e) => setTtl(Number(e.target.value))}>
                {ttlChoices.map((t) => (
                  <option key={t.secs} value={t.secs}>
                    {t.label}
                  </option>
                ))}
              </Select>
            )}
          />
          <FieldRow
            label="Uses"
            help={usesFixed ? "A UI code works once." : "More than one only for a fleet that shares a code, such as per-pod init containers."}
            error={shown("uses")}
            render={(a) => (
              <Input {...a} type="number" min={1} max={1000} disabled={usesFixed} value={effectiveUses} onChange={(e) => setUses(Number(e.target.value))} className="w-24" />
            )}
          />
          {stepUpRole && (
            <FieldRow
              label="Step-up secret"
              required
              help={`${role === "proxy" ? "This code hands out the cluster keyring" : "This code hands out the admin token"}. Type the controller's step-up secret again; it is sent with this request only and kept nowhere.`}
              error={stepUpError ?? shown("stepUp")}
              render={(a) => (
                <Input {...a} type="password" autoComplete="new-password" data-1p-ignore data-lpignore="true" data-form-type="other" value={stepUp} onChange={(e) => setStepUp(e.target.value)} className="w-full max-w-xl" />
              )}
            />
          )}
          <FieldRow
            label="Keep the code out of the command"
            help="The command carries no code; you paste it when the node asks. Use it if shell history or process lists are shared."
            render={(a) => (
              <input {...a} type="checkbox" checked={prompt} onChange={(e) => setPrompt(e.target.checked)} />
            )}
          />
          {formError && <ErrorState>{formError}</ErrorState>}
          <div className="mt-3 flex items-center gap-3">
            <Button type="submit" variant="primary" loading={minting} disabled={tried && !valid}>
              Create code
            </Button>
            {tried && !valid && <span role="status" className="text-xs text-bad">Fix the fields marked above.</span>}
          </div>
        </form>
      )}

      <h2 className="mt-8 mb-2 text-base font-semibold">Codes</h2>
      {notice && (
        <p role="status" className="mb-2 text-sm text-ink-muted">
          {notice}
        </p>
      )}
      {listError && <ErrorState onRetry={refresh}>{listError}</ErrorState>}
      {codes === null && !listError && <Loading rows={2} />}
      {codes && <PendingCodes items={codes} canWrite={canWrite} busy={busy} onRevoke={handleRevoke} />}
      {dialog}
    </div>
  );
}
