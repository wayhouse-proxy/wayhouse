// Builds the one-command setup for a freshly minted enrollment code. Pure text in, text out:
// nothing here touches the network, storage or the clipboard. The code appears only in a step
// marked `holdsCode`, and only when the operator did not choose the prompt form.

export type EnrollRole = "agent" | "proxy" | "aggregator" | "ui" | "controller";
export type Target = "systemd" | "docker" | "compose" | "k8s";

export const ROLES: { role: EnrollRole; label: string; what: string }[] = [
  { role: "agent", label: "Origin agent", what: "Runs next to a game server and opens its WireGuard tunnel." },
  { role: "proxy", label: "Proxy", what: "A wayhouse data-plane node. Receives the cluster keyring." },
  { role: "aggregator", label: "Aggregator", what: "Collects fleet state from proxies." },
  { role: "ui", label: "UI", what: "Another admin UI. Receives the admin token." },
  {
    role: "controller",
    label: "Controller replica",
    what: "Joins this controller's HA group. Receives every token, the keyring and the Raft credential.",
  },
];

export const TARGETS: { target: Target; label: string }[] = [
  { target: "systemd", label: "systemd" },
  { target: "docker", label: "docker run" },
  { target: "compose", label: "Docker Compose" },
  { target: "k8s", label: "Kubernetes" },
];

/** Roles whose grant is the keyring or the admin token: minting needs the step-up secret. */
export const needsStepUp = (role: EnrollRole) => role === "proxy" || role === "ui" || role === "controller";

/** Why `role` cannot go to `target` yet, or null. */
export function unsupported(role: EnrollRole, target: Target): string | null {
  if (role === "controller" && target !== "systemd" && target !== "k8s") {
    return "A controller replica is installed with systemd, or on Kubernetes as a StatefulSet that shares one pool code.";
  }
  if (role === "agent" && target === "k8s") {
    return "An agent on Kubernetes needs the per-pod init container, which is not shipped yet. Use systemd, docker run or Compose on the origin host.";
  }
  return null;
}

const BIN: Record<EnrollRole, string> = {
  agent: "wayhouse-agent",
  proxy: "wayhouse",
  aggregator: "wayhouse-aggregator",
  ui: "wayhouse-ui",
  controller: "wayhouse-controller",
};

export interface Spec {
  role: EnrollRole;
  target: Target;
  /** The node's name, as minted. */
  name: string;
  controllerUrl: string;
  /** `sha256:<64 hex>` */
  pin: string;
  /** `null` is the prompt form: the command carries no code and the node asks for it. */
  code: string | null;
  imageTag: string;
  /** Agent: `host:port` or `:port`, comma separated. */
  backends: string;
  /** Proxy and UI: where the aggregator listens. */
  aggregatorUrl: string;
  /** Aggregator: the parent tier, as minted. */
  parentUrl: string;
  /** Controller replica: how the other replicas reach it, `https://host:port`. */
  advertise: string;
  namespace: string;
  /** Kubernetes controller pool: how many replicas the code covers. */
  uses?: number;
}

export interface Step {
  title: string;
  text: string;
  language: "sh" | "yaml";
  /** The text contains the one-time code. */
  holdsCode: boolean;
  note?: string;
}

// --- validation (mirrors the controller, so the operator hears it before minting) ---

export const validName = (s: string) => /^[A-Za-z0-9._-]{1,64}$/.test(s) && s !== "subscribe";
export const validPin = (s: string) => /^sha256:[0-9a-fA-F]{64}$/.test(s);
export const validTag = (s: string) => /^[A-Za-z0-9_][A-Za-z0-9_.-]{0,127}$/.test(s);
/** Like the enroll CLI's check, plus what a replica needs: https and an explicit port. */
// shortcut: mirrors the CLI check (no whitespace, control, quote, backslash, % or $; 256 bytes max) by hand.
export const validAdvertise = (s: string) => s.length <= 256 && !/[\x00-\x1f\x7f]/.test(s) && validUrl(s, { httpsOnly: true }) && /^https:\/\/[^/]+:\d{1,5}\/?$/.test(s) && !s.includes("%");
/** The pod name prefix of the `ha-enrolled` overlay's replica StatefulSet; a pool code is minted with it. */
export const POOL_PREFIX = "wayhouse-controller-replica-";
export const validNamespace = (s: string) => /^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$/.test(s);

export function validUrl(s: string, opts: { httpsOnly?: boolean } = {}): boolean {
  if (s.length > 512 || /[\s"'\\`$]/.test(s)) return false;
  try {
    const u = new URL(s);
    if (u.username || u.password || !u.hostname) return false;
    if (u.protocol === "https:") return true;
    return u.protocol === "http:" && !opts.httpsOnly;
  } catch {
    return false;
  }
}

export const validBackends = (s: string) =>
  s.split(",").every((b) => /^[A-Za-z0-9.:[\]-]*:\d{1,5}$/.test(b.trim())) && s.trim() !== "";

// --- quoting ---

/** A word safe in a POSIX shell: bare when it only has safe characters, else single-quoted. */
export function sh(s: string): string {
  return /^[A-Za-z0-9_@%+=:,./-]+$/.test(s) ? s : `'${s.replace(/'/g, `'\\''`)}'`;
}

const y = (s: string) => JSON.stringify(s);

// --- steps ---

const imageFor = (r: EnrollRole, tag: string) => `ghcr.io/wayhouse-proxy/${BIN[r]}:${tag}`;
const dirFor = (name: string) => `wayhouse-${name}`;

/** The flags after `--` (systemd) or after the image (docker, compose, Kubernetes). */
function runFlags(s: Spec): string[] {
  switch (s.role) {
    case "agent":
      return [`--backends=${s.backends}`];
    case "proxy":
      return s.aggregatorUrl ? [`--aggregator=${s.aggregatorUrl}`] : [];
    case "ui":
      return s.aggregatorUrl ? [`--aggregator-url=${s.aggregatorUrl}`] : [];
    case "aggregator":
    case "controller":
      return [];
  }
}

/** What the node binary is started with in a container, the enrolled files being in /data. */
function containerArgs(s: Spec, secretsDir: string): string[] {
  switch (s.role) {
    case "agent":
      return ["--data-dir=/data", ...runFlags(s)];
    case "proxy":
      return [
        `--controller=${s.controllerUrl}`,
        `--aggregator-instance=${s.name}`,
        `--secret-key-file=/data/secret.keys`,
        ...runFlags(s),
      ];
    case "aggregator":
      return ["--listen=0.0.0.0:9902", ...(s.parentUrl ? [`--parent-url=${s.parentUrl}`] : [])];
    case "ui":
      return [
        "--listen=0.0.0.0:9903",
        `--controller-url=${s.controllerUrl}`,
        `--ui-password-file=${secretsDir}/ui-password`,
        ...runFlags(s),
      ];
    case "controller":
      return [];
  }
}

/** The replica listens on the port it advertises; the certificate files are the operator's to place. */
function controllerFlags(s: Spec): string[] {
  const port = /:(\d+)\/?$/.exec(s.advertise)?.[1] ?? "9901";
  return [
    `--listen=0.0.0.0:${port}`,
    `--tls-cert=/etc/wayhouse/${s.name}.pem`,
    `--tls-key=/etc/wayhouse/${s.name}.key`,
    `--ca-file=/etc/wayhouse/ca.pem`,
  ];
}

const enrollFlags = (s: Spec) => [`--controller-url=${s.controllerUrl}`, `--pin=${s.pin}`];

function systemd(s: Spec): Step[] {
  const bin = BIN[s.role];
  const lines = [`sudo ${s.code ? `env WAYHOUSE_ENROLL_CODE=${sh(s.code)} ` : ""}${bin} install --systemd`];
  lines.push(...enrollFlags(s).map((f) => sh(f)));
  if (s.role === "controller") lines.push(`--advertise=${sh(s.advertise)}`);
  if (s.role === "ui") lines.push(`--credential ui-password=/etc/wayhouse/ui/ui-password`);
  const extra = s.role === "controller" ? controllerFlags(s) : runFlags(s);
  if (extra.length) lines.push("--", ...extra.map(sh));
  const steps: Step[] = [];
  if (s.role === "ui") {
    steps.push({
      title: "Create the login password first",
      language: "sh",
      holdsCode: false,
      text: `sudo install -d -m 700 /etc/wayhouse/ui\nhead -c 24 /dev/urandom | base64 | sudo tee /etc/wayhouse/ui/ui-password >/dev/null\nsudo chmod 600 /etc/wayhouse/ui/ui-password`,
      note: "The UI's login password is not part of enrollment. This writes a random one; read it from that file.",
    });
  }
  steps.push({
    title: "Enroll and start the service (as root, with the binary installed)",
    language: "sh",
    holdsCode: s.code !== null,
    text: lines.join(" \\\n  "),
    note:
      (s.role === "controller"
        ? `Put this replica's certificate and key at /etc/wayhouse/${s.name}.pem and .key and the cluster CA at /etc/wayhouse/ca.pem first, or change the paths. The service runs as a dynamic user, so the key must be readable by it (a root-only 0600 key is not). `
        : "") +
      (s.code
        ? "The code is not on the node binary's command line, but it is in this command, so it is in your shell history and briefly in the process list. Tick \"Keep the code out of the command\" to avoid that."
        : "The command asks for the code on the terminal; paste it there."),
  });
  return steps;
}

const dockerUser = (r: EnrollRole) => (r === "agent" ? "0" : `"$(id -u):$(id -g)"`);

function dockerEnroll(s: Spec): Step {
  const dir = dirFor(s.name);
  const parts = [
    `mkdir -p ${sh(`./${dir}`)}`,
    [
      `docker run --rm${s.code ? "" : " -it"} --user ${dockerUser(s.role)}`,
      `-v "$PWD/${dir}:/data"`,
      ...(s.code ? [`-e WAYHOUSE_ENROLL_CODE=${sh(s.code)}`] : []),
      imageFor(s.role, s.imageTag),
      "enroll --data-dir /data",
      ...enrollFlags(s).map((f) => sh(f)),
    ].join(" \\\n  "),
  ];
  return {
    title: "Enroll into a local directory (one shot)",
    language: "sh",
    holdsCode: s.code !== null,
    text: parts.join("\n"),
    note: s.code
      ? "docker inspect shows the code in this container's environment, but it is spent once this ends."
      : "The container asks for the code on the terminal; paste it there.",
  };
}

function dockerRun(s: Spec): Step[] {
  const dir = dirFor(s.name);
  const img = imageFor(s.role, s.imageTag);
  const base = [`docker run -d --name wayhouse-${s.role}-${s.name} --restart unless-stopped`, `--read-only --security-opt no-new-privileges:true --cap-drop ALL`];
  const steps: Step[] = [];
  let mid: string[];
  switch (s.role) {
    case "controller":
      return [];
    case "agent":
      mid = [`--network host --user 0 --cap-add NET_ADMIN --cap-add DAC_OVERRIDE --device /dev/net/tun`, `--tmpfs /tmp`, `-v "$PWD/${dir}:/data"`];
      break;
    case "proxy":
      mid = [`--network host --user ${dockerUser("proxy")}`, `--tmpfs /tmp`, `-e CREDENTIALS_DIRECTORY=/data`, `-v "$PWD/${dir}:/data:ro"`];
      break;
    case "aggregator":
      mid = [`--user ${dockerUser("aggregator")}`, `-p 9902:9902`, `--tmpfs /tmp`, `-e CREDENTIALS_DIRECTORY=/data`, `-v "$PWD/${dir}:/data:ro"`];
      break;
    case "ui":
      steps.push({
        title: "Create the login password",
        language: "sh",
        holdsCode: false,
        text: `(umask 077; head -c 24 /dev/urandom | base64 > ${sh(`./${dir}/ui-password`)})`,
        note: "The UI's login password is not part of enrollment. This writes a random one; read it from that file.",
      });
      mid = [`--user ${dockerUser("ui")}`, `-p 9903:9903`, `--tmpfs /tmp`, `-e CREDENTIALS_DIRECTORY=/data`, `-v "$PWD/${dir}:/data:ro"`];
      break;
  }
  const args = containerArgs(s, "/data").map(sh);
  steps.push({
    title: "Run it",
    language: "sh",
    holdsCode: false,
    text: [...base, ...mid, img, ...args].join(" \\\n  "),
    note: "Game ports below 1024 need more than this (root or a sysctl); set that up as you do for any proxy.",
  });
  return steps;
}

function composeFile(s: Spec): string {
  const img = imageFor(s.role, s.imageTag);
  const root = s.role !== "proxy";
  const L: string[] = ["services:"];
  const common = (extra: string[]) => [
    `    image: ${y(img)}`,
    ...extra,
    `    read_only: true`,
    `    tmpfs: ["/tmp"]`,
    `    security_opt: ["no-new-privileges:true"]`,
    `    cap_drop: [ALL]`,
  ];
  // The one-shot enrollment: the code is a Docker secret file, never an argument.
  L.push(
    `  enroll:`,
    ...common([`    profiles: ["enroll"]`, ...(root ? [`    user: "0"`] : [])]),
    ...(s.role === "agent" ? [`    cap_add: [DAC_OVERRIDE]`] : []),
    `    command: ${JSON.stringify(["enroll", "--data-dir=/data", ...enrollFlags(s)])}`,
    `    environment:`,
    `      WAYHOUSE_ENROLL_CODE_FILE: /run/secrets/enroll-code`,
    `    secrets: [enroll-code]`,
    `    volumes: ["data:/data"]`,
  );
  const node: string[] = [`  ${s.role}:`];
  const mount = s.role === "agent" ? `"data:/data"` : `"data:/data:ro"`;
  switch (s.role) {
    case "agent":
      node.push(
        ...common([`    restart: unless-stopped`, `    user: "0"`, `    network_mode: host`]),
        `    cap_add: [NET_ADMIN, DAC_OVERRIDE]`,
        `    devices: ["/dev/net/tun:/dev/net/tun"]`,
      );
      break;
    case "proxy":
      node.push(...common([`    restart: unless-stopped`, `    network_mode: host`, `    environment:`, `      CREDENTIALS_DIRECTORY: /data`]));
      break;
    case "aggregator":
      node.push(...common([`    restart: unless-stopped`, `    user: "0"`, `    ports: ["9902:9902"]`, `    environment:`, `      CREDENTIALS_DIRECTORY: /data`]));
      break;
    case "ui":
      node.push(
        ...common([`    restart: unless-stopped`, `    user: "0"`, `    ports: ["9903:9903"]`, `    environment:`, `      CREDENTIALS_DIRECTORY: /data`]),
        `    secrets: [ui-password]`,
      );
      break;
  }
  node.push(`    command: ${JSON.stringify(containerArgs(s, "/run/secrets"))}`, `    volumes: [${mount}]`);
  L.push(...node, `secrets:`, `  enroll-code:`, `    file: ${y(`./${s.name}.enroll`)}`);
  if (s.role === "ui") L.push(`  ui-password:`, `    file: ${y(`./${s.name}.ui-password`)}`);
  L.push(`volumes:`, `  data: {}`);
  return L.join("\n");
}

function compose(s: Spec): Step[] {
  const f = `compose.${s.name}.yml`;
  const steps: Step[] = [
    {
      title: `Save the code as ${s.name}.enroll (mode 0600)`,
      language: "sh",
      holdsCode: s.code !== null,
      text: s.code
        ? `(umask 077; printf '%s\\n' ${sh(s.code)} > ${sh(`${s.name}.enroll`)})`
        : `(umask 077; cat > ${sh(`${s.name}.enroll`)})`,
      note: s.code
        ? undefined
        : "Paste the code, press Enter, then Ctrl-D. It never appears in the shell history.",
    },
  ];
  if (s.role === "ui") {
    steps.push({
      title: "Create the login password",
      language: "sh",
      holdsCode: false,
      text: `(umask 077; head -c 24 /dev/urandom | base64 > ${sh(`${s.name}.ui-password`)})`,
      note: "The UI's login password is not part of enrollment. This writes a random one; read it from that file.",
    });
  }
  steps.push(
    { title: f, language: "yaml", holdsCode: false, text: composeFile(s), note: "Holds no secret: the code is a Docker secret from the file above." },
    {
      title: "Enroll, start, then delete the spent code",
      language: "sh",
      holdsCode: false,
      text: `docker compose -f ${sh(f)} run --rm enroll\ndocker compose -f ${sh(f)} up -d ${s.role}\nrm -- ${sh(`${s.name}.enroll`)}`,
    },
  );
  return steps;
}

/** The `ha-enrolled` overlay: replica pods each redeem one pool code from the `wayhouse-enroll-code` Secret. */
function k8sControllerPool(s: Spec): Step[] {
  const ns = sh(s.namespace);
  return [
    {
      title: "Create the step-up Secret (once, by hand)",
      language: "sh",
      holdsCode: false,
      text: `kubectl -n ${ns} create secret generic wayhouse-enroll-step-up --from-file=step-up=./step-up`,
      note: "The overlay also needs the wayhouse-tls, wayhouse-tokens and wayhouse-keyring Secrets that `wayhouse-controller init --target k8s-secret` makes; create them first. Skip this step if the step-up Secret exists. It must hold the same step-up secret you typed above, in a file named step-up; the bootstrap controller and every replica read it.",
    },
    {
      title: "Store the pool code as a Secret",
      language: "sh",
      holdsCode: s.code !== null,
      text: s.code
        ? `printf '%s' ${sh(s.code)} | kubectl -n ${ns} create secret generic wayhouse-enroll-code --from-file=code=/dev/stdin`
        : `kubectl -n ${ns} create secret generic wayhouse-enroll-code --from-file=code=/dev/stdin`,
      note: s.code
        ? "Every replica pod reads this one code. This line puts the code in your shell history; tick \"Keep the code out of the command\" to paste it at a prompt instead. Delete the Secret once the pods have joined."
        : "Paste the code, press Enter, then Ctrl-D. It never appears in the shell history.",
    },
    {
      title: "Apply the overlay and start the replicas",
      language: "sh",
      holdsCode: false,
      text: `kubectl apply -k deploy/k8s/overlays/ha-enrolled
kubectl -n ${ns} scale statefulset/wayhouse-controller-replica --replicas=${s.uses ?? 2}`,
      note: `The overlay installs into the wayhouse namespace. The overlay starts with replicas: 2 and applying it again resets the count to 2, so scale after every apply. Each new pod spends one of the code's ${s.uses ?? 2} uses, so never scale past that; a pod that keeps its volume spends none when it restarts.`,
    },
  ];
}

function k8s(s: Spec): Step[] {
  if (s.role === "controller") return k8sControllerPool(s);
  const secret = `wayhouse-${s.name}`;
  const ns = sh(s.namespace);
  const dir = dirFor(s.name);
  const args = containerArgs(s, "/run/wayhouse");
  const fragment = [
    `# container`,
    `args: ${JSON.stringify(args)}`,
    `env:`,
    `  - { name: CREDENTIALS_DIRECTORY, value: /run/wayhouse }`,
    `volumeMounts:`,
    `  - { name: enrolled, mountPath: /run/wayhouse, readOnly: true }`,
    `# pod`,
    `volumes:`,
    `  - name: enrolled`,
    `    secret: { secretName: ${y(secret)}, defaultMode: 0400 }`,
  ].join("\n");
  const steps: Step[] = [
    dockerEnroll(s),
    {
      title: "Store the result as a Secret, then delete the local copy",
      language: "sh",
      holdsCode: false,
      text: `kubectl -n ${ns} create secret generic ${sh(secret)} --from-file=${sh(`./${dir}`)}\nrm -r ${sh(`./${dir}`)}`,
      note: "No pod ever holds the code. These two steps become one command once `enroll --output k8s-secret` ships.",
    },
  ];
  if (s.role === "ui") {
    steps.push({
      title: "Create the login password Secret",
      language: "sh",
      holdsCode: false,
      text: `head -c 24 /dev/urandom | base64 | kubectl -n ${ns} create secret generic ${sh(`${secret}-ui-password`)} --from-file=ui-password=/dev/stdin`,
      note: "Mount it next to the enrolled Secret and point --ui-password-file at it.",
    });
  }
  steps.push({
    title: "Mount it in the workload",
    language: "yaml",
    holdsCode: false,
    text: fragment,
    note: "Add to the container and pod spec of the existing deploy/k8s manifest for this role. Keep the TLS Secret mount that manifest already has.",
  });
  return steps;
}

export function generate(s: Spec): Step[] {
  switch (s.target) {
    case "systemd":
      return systemd(s);
    case "docker":
      return [dockerEnroll(s), ...dockerRun(s)];
    case "compose":
      return compose(s);
    case "k8s":
      return k8s(s);
  }
}
