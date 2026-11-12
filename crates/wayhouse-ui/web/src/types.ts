// Wire shapes from wayhouse-aggregator (relayed verbatim by wayhouse-ui's /ws/fleet
// and GET /api/fleet/*) and wayhouse-controller (proxied by GET/POST
// /api/config*). Kept in one file since both sides are simple, stable JSON
// this frontend only ever reads or round-trips, never transforms.

export interface BackendSummary {
  addr: string;
  healthy: boolean;
  state: string; // "enabled" | "draining" | "disabled"
  active: number;
}

export interface PoolSummary {
  name: string;
  balancer: string;
  backends: BackendSummary[];
}

export interface SessionCounts {
  tcp: number;
  udp: number;
}

export interface FleetInstanceView {
  instance: string;
  last_seen_ms_ago: number;
  stale: boolean;
  pools: PoolSummary[];
  sessions: SessionCounts;
  /** Self-reported fleet organization path, e.g. "eu/frankfurt/cluster-a". */
  group?: string | null;
  /** Product version the instance runs; empty from a build that predates it. */
  version?: string;
  /** Component wire protocol `major.minor`; empty from an older build. */
  protocol?: string;
  /** Against the newest version in the fleet (the aggregator computes it). */
  skew?: "none" | "within-window" | "outside-window";
}

export interface FanoutInstanceResult {
  instance: string;
  status: number | null;
  error: string | null;
}

export interface FanoutResponse {
  results: FanoutInstanceResult[];
}

export interface RevisionSummary {
  revision: number;
  size_bytes: number;
  current: boolean;
  /** False while a canary revision is visible only to `canary_groups`. */
  promoted?: boolean;
  canary_groups?: string[];
}

export interface SnifferInfo {
  name: string;
  sha256: string;
  size_bytes: number;
  loaded: boolean;
  /** A previous version is kept and can be rolled back to. */
  has_previous?: boolean;
  /** The live module is the previous version because the current file failed validation. */
  fallback?: boolean;
}

/** One row of wayhouse-controller's `GET /tunnel/addresses` (`docs/11` "Address authority"). */
export interface TunnelAddressEntry {
  role: "origin" | "proxy";
  name: string;
  address: string;
  /** Unix seconds. */
  first_seen: number;
  /** Unix seconds of the last registration. */
  last_seen: number;
  /** Not re-registered for longer than the controller's `--tunnel-stale-after`. */
  stale: boolean;
}

export interface TunnelAddresses {
  /** `null` when the controller runs without `--tunnel-network` (pinned addresses only). */
  network: string | null;
  allocated: number;
  capacity: number | null;
  entries: TunnelAddressEntry[];
}

/** One sniffer registry the UI knows (`GET /api/registries`). */
export interface RegistryRef {
  id: string;
  name: string;
  url: string;
  official: boolean;
  risk: "official" | "external";
}

export interface RegistryList {
  registries: RegistryRef[];
  /** False when the list lives in memory only and is lost on restart. */
  persistent: boolean;
}

export interface RegistryVersion {
  version: string;
  abi: string;
  min_proxy: string;
  url: string;
  sha256: string;
  size: number;
  signature_url?: string;
  /** True when installing this version checks its signature (official registry with a key). */
  verifiable?: boolean;
  limits: { max_memory_bytes: number; call_timeout_ms: number };
  config?: string;
}

/** `compatible` has `version` when this release can run it, else `reason`. */
export interface RegistrySniffer {
  name: string;
  description: string;
  license: string;
  homepage?: string;
  versions: RegistryVersion[];
  /** True when installing the chosen version checks its signature (official registry with a key). */
  verifiable?: boolean;
  compatible: { version: string } | { reason: string };
}

/** What a catalog entry carries without its versions (the list of `GET /api/registries/{id}/sniffers`). */
export interface RegistryCatalogEntry {
  name: string;
  description: string;
  license: string;
  /** The registry validates it as https at parse time, but a UI that renders it as a link must still allow https only. */
  homepage?: string;
  categories: string[];
  tags: string[];
  /** Newest version. */
  latest: string;
  abi: string;
  /** The newest version lists a signature. A hint: installing checks the version itself. */
  signed: boolean;
  /** YYYY-MM-DD */
  updated: string;
  /** Only when the registry publishes download counts. */
  downloads?: number;
  /** Built for this release's ABI (the newest version only; the detail has the full check). */
  abi_compatible: boolean;
}

export interface RegistrySniffers {
  registry: RegistryRef;
  index_name: string;
  /** Where the registry publishes download counts, if it does. */
  stats_url?: string | null;
  host_abi: string;
  sniffers: RegistryCatalogEntry[];
}

/** `GET /api/registries/{id}/sniffers/{name}`: one sniffer with every version. */
export interface RegistrySnifferDetail {
  registry: RegistryRef;
  sniffer: RegistrySniffer;
  host_abi: string;
  min_proxy_checked: boolean;
}

export interface InstallInstanceResult {
  instance: string;
  ok: boolean;
  pinned: boolean;
  error: string | null;
  status: number | null;
  /** The start of a refusing proxy's reply. */
  detail?: string | null;
}

export interface InstallResponse {
  sniffer: string;
  version: string;
  signed: boolean;
  risk: "official" | "external";
  sha256: string;
  results: InstallInstanceResult[];
  pinned_instances: { instance: string; pin: { name: string; sha256: string } }[];
}

/** A newer version of an installed sniffer (`POST /api/registries/updates/check`). */
export interface SnifferUpdate {
  registry_id: string;
  version: string;
  compatible: boolean;
  reason: string | null;
}

/** Instances that run one build of one sniffer. */
export interface UpdateCheckRow {
  sniffer: string;
  installed_sha256: string;
  /** Null for an unknown build: its hash is in no registry index. */
  installed_version: string | null;
  known: boolean;
  has_previous: boolean;
  fallback: boolean;
  update: SnifferUpdate | null;
  instances: string[];
}

export interface UpdateCheck {
  host_abi: string;
  min_proxy_checked: boolean;
  registries: { id: string; name: string; ok: boolean; error?: string }[];
  sniffers: UpdateCheckRow[];
  instance_errors: { instance: string; status: number; detail: string }[];
}

// --- plugins (proxied to wayhouse-controller's /plugins, slice 3) ---

export interface PluginCapabilities {
  triggers: { on_timer?: boolean; on_webhook?: boolean; on_event?: string[]; on_acme_dns?: boolean };
  tick_interval_secs: number;
  log: boolean;
  state: { max_bytes: number } | null;
  /** HTTPS endpoints the plugin may call. */
  http?: { hosts: { host: string; port?: number; allow_private?: boolean }[] } | null;
  /** Secret slots it reads in request headers, each bound to hosts of `http`. */
  secrets?: { name: string; description: string; hosts: string[] }[];
  /** Hostnames it may claim and the networks they may point into. */
  routes?: { hosts: string[]; backends: string[]; max_entries: number } | null;
  /** Entry ports a plugin may be given for its servers (`port_routes`, docs/plugins.md). */
  port_routes?: {
    ports: string[];
    proto: ("udp" | "tcp")[];
    backends: string[];
    max_entries: number;
  } | null;
}

/** What a module declares for install-time options (docs/plugins.md, "Install options"). */
export interface PluginManifest {
  name: string;
  /** A JSON Schema subset; `x-wayhouse-secret` marks a value set under Secrets, not in the form. */
  options: Record<string, any>;
  bindings: { option: string; kind: string }[];
}

export interface PluginModule {
  sha256: string;
  size: number;
  abi: string;
  /** Only what does not depend on the options when the module has a manifest. */
  capabilities: PluginCapabilities;
  manifest?: PluginManifest | null;
}

export interface PluginInstall {
  id: string;
  name: string;
  sha256: string;
  size: number;
  approved: PluginCapabilities;
  config: unknown;
  enabled: boolean;
  created_at: number;
  created_by: string | null;
  /** Present while the install's webhook is enabled. */
  webhook?: unknown;
}

/** One line of the plugins-file dry run (`GET /api/plugins/file/plan`). */
export interface PluginsFileRow {
  name: string;
  id: string | null;
  action: "create" | "update" | "remove" | "unchanged" | "refused";
  reason?: string;
  /** For a refusal over the approval hash: the real hash, to paste into the file. */
  effective_sha256?: string;
  error?: string;
}

export interface PluginsFilePlan {
  applied: boolean;
  plugins: PluginsFileRow[];
}

/** `GET /api/plugins/{id}`: the install with its module's manifest. */
export interface PluginDetail extends PluginInstall {
  manifest: PluginManifest | null;
}

export interface PluginSecretSlot {
  slot: string;
  description: string;
  bound_hosts: string[];
  set: boolean;
  /** `unset`, `ok` or `held: <reason>`. */
  status: string;
  key_id?: string | null;
  updated_at?: number | null;
}

export interface PluginStatus {
  ticks: number;
  last_tick_unix: number | null;
  last_ok: boolean | null;
  last_error: string | null;
  consecutive_failures: number;
  held: string | null;
  hook_calls?: number;
  hook_failures?: number;
  hooks_dropped?: number;
  logs: { level: string; msg: string }[];
}

/** One entry-port route of the plugin overlay (`GET /api/plugin-routes`). */
export interface PluginPortRoute {
  key: string;
  port: number;
  proto: "udp" | "tcp" | "both";
  backend: string;
  owner: string;
  plugin: string;
}

/** A port route still waiting for a free entry port. */
export interface PluginExhaustedPort {
  key: string;
  backend: string;
  owner: string;
  plugin: string;
}

export interface PluginRouteOverlay {
  routes: unknown[];
  conflicts: unknown[];
  port_routes?: PluginPortRoute[];
  exhausted?: PluginExhaustedPort[];
}

/** One ACME site (`GET /api/acme/sites`, proxied from the controller). */
export interface AcmeSite {
  name: string;
  names: string[];
  gateways: string[];
  challenge: "dns-01" | "http-01";
  dns_plugin: string;
  enabled: boolean;
  challenge_zone: string | null;
  propagation_timeout_secs: number;
  certificate: {
    version: number;
    /** Unix seconds. */
    not_after: number;
    issued_at: number;
    serial: string;
    names: string[];
    /** Id of the cluster secret key the private key is sealed with. */
    key_id: string;
    /** Why this controller cannot unseal the key, or `null` when it can. */
    held: string | null;
    /** Issued by a staging CA: browsers will not trust it. */
    untrusted: boolean;
  } | null;
  attempt: { failures: number; next_attempt_at: number; last_error: string | null } | null;
}

export interface AcmeGateway {
  name: string;
  created_at: number;
  sites: string[];
}

export interface AcmeAccount {
  directory_url: string;
  account_url: string | null;
  /** Id of the cluster secret key the account credentials are sealed with. */
  key_id?: string;
}

/** One row of the controller's `GET /features` (`wayhouse_config::features::FeatureView`). */
export interface FeatureFlag {
  name: string;
  value: boolean;
  source: "default" | "config" | "override" | "build";
  default: boolean;
  scope: "proxy" | "controller";
  apply: "live" | "startup";
  override: "both" | "off_only";
  protection: boolean;
  wired: boolean;
  available: boolean;
  since: number;
  risk: string;
  impact: string;
}

/** One flag across the fleet (`wayhouse_aggregator::features::FlagRollup`). */
export interface FlagRollup {
  name: string;
  on: number;
  off: number;
  unavailable: number;
  off_instances: string[];
  overridden_instances: string[];
}

export interface FeatureRollup {
  reporting: number;
  total: number;
  flags: FlagRollup[];
}

export interface FeatureOverride {
  name: string;
  value: boolean;
  reason: string;
  actor?: string;
  set_at: number;
  expires_at?: number;
}

/** One row of the controller's `GET /secrets`: a name and when it was set, never a value. */
export interface ClusterSecret {
  name: string;
  key_id: string;
  set_at: number;
  actor?: string;
}

/** One version in a plugin registry's index (`GET /api/plugin-registries/{id}/plugins`). */
export interface RegistryPluginVersion {
  version: string;
  abi: string;
  size: number;
  signature_url?: string;
  /** A display hint for the options form; the module's own manifest stays authoritative. */
  options_schema?: { properties?: Record<string, { title?: string; description?: string; type?: string }> } | null;
  /** Built for this controller's plugin ABI. */
  compatible: boolean;
  /** True when the fetch checks this version's signature (official registry with a key). */
  verifiable: boolean;
}

export interface RegistryPlugin {
  name: string;
  description: string;
  license: string;
  homepage?: string;
  versions: RegistryPluginVersion[];
  compatible: boolean;
}

/** A plugin catalog entry, without versions (the list of `GET /api/plugin-registries/{id}/plugins`). */
export interface RegistryPluginCatalogEntry {
  name: string;
  description: string;
  license: string;
  homepage?: string;
  categories: string[];
  tags: string[];
  latest: string;
  abi: string;
  signed: boolean;
  updated: string;
  downloads?: number;
  abi_compatible: boolean;
}

export interface RegistryPlugins {
  registry: RegistryRef;
  index_name: string;
  stats_url?: string | null;
  host_abi: string;
  plugins: RegistryPluginCatalogEntry[];
}

/** `GET /api/plugin-registries/{id}/plugins/{name}` */
export interface RegistryPluginDetail {
  plugin: RegistryPlugin;
  host_abi: string;
}

/** What a registry fetch answers: the module is uploaded and inspected, nothing is installed. */
export interface PluginFetchResult {
  plugin: string;
  version: string;
  signed: boolean;
  risk: "official" | "external";
  upload: PluginModule;
}

/** One flag of a fleet program, as the program itself published it (`GET /api/process`). */
export interface ProcessFlag {
  name: string;
  help: string;
  kind: "bool" | "text" | "list";
  value_name: string | null;
  default: string | null;
  editable: boolean;
  /** Why it cannot be edited here. */
  reason: string | null;
}

/** A settings value: a switch, one text, or a list. */
export type ProcessValue = boolean | string | string[];

export interface ProcessFile {
  rev: number;
  values: Record<string, ProcessValue>;
  previous: Record<string, ProcessValue> | null;
  /** A Viewer sees that a change is waiting but not the values (empty here). */
  redacted?: boolean;
  pending: { applied_at: number; attempts: number; confirm_secs: number } | null;
  /** Why the program itself refused the last change. */
  error: string | null;
}

export interface ProcessProgram {
  program: string;
  label: string;
  /** The program has started with `--settings-dir` and published its flags. */
  connected: boolean;
  flags: ProcessFlag[];
  file: ProcessFile | null;
}

export interface ProcessList {
  dir_configured: boolean;
  programs: ProcessProgram[];
}

/** One row of the controller's `GET /enrollments`: a code's state, never the code itself. */
export interface EnrollmentCode {
  id: string;
  role: "agent" | "proxy" | "aggregator" | "ui";
  /** The node's name. */
  origin: string;
  created_at: number;
  expires_at: number;
  uses: number;
  uses_left: number;
  status: "pending" | "used" | "expired";
  created_by?: string;
  parent_url?: string;
}

/** The answer to a mint. The `code` is in this one response and nowhere else. */
export interface MintedEnrollment {
  id: string;
  code: string;
  role: EnrollmentCode["role"];
  name: string;
  expires_at: number;
  uses: number;
}
