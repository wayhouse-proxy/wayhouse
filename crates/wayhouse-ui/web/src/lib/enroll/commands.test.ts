import { describe, expect, it } from "vitest";
import {
  generate,
  sh,
  unsupported,
  validBackends,
  validName,
  validPin,
  validUrl,
  type EnrollRole,
  type Spec,
  type Target,
} from "./commands";

const CODE = "abc123.SECRETSECRETSECRETSECRETSECRETSE";
const PIN = `sha256:${"ab".repeat(32)}`;

const spec = (role: EnrollRole, target: Target, over: Partial<Spec> = {}): Spec => ({
  role,
  target,
  name: "edge-1",
  controllerUrl: "https://ctl.example:9901",
  pin: PIN,
  code: CODE,
  imageTag: "0.4.0",
  backends: ":25565",
  aggregatorUrl: "",
  parentUrl: "",
  namespace: "wayhouse",
  ...over,
});

const ROLES: EnrollRole[] = ["agent", "proxy", "aggregator", "ui"];
const TARGETS: Target[] = ["systemd", "docker", "compose", "k8s"];
const combos = ROLES.flatMap((r) => TARGETS.map((t) => [r, t] as const)).filter(([r, t]) => !unsupported(r, t));

describe("generate", () => {
  it.each(combos)("%s on %s puts the code only in steps that say they hold it", (role, target) => {
    const steps = generate(spec(role, target));
    const holding = steps.filter((s) => s.text.includes(CODE));
    expect(holding.length).toBe(1);
    expect(holding.every((s) => s.holdsCode)).toBe(true);
    expect(steps.filter((s) => s.holdsCode && !s.text.includes(CODE))).toEqual([]);
  });

  it.each(combos)("%s on %s has no code anywhere in the prompt form", (role, target) => {
    const steps = generate(spec(role, target, { code: null }));
    for (const s of steps) {
      expect(s.holdsCode).toBe(false);
      expect(s.text).not.toContain("SECRET");
    }
  });

  it.each(combos)("%s on %s pins the controller and never passes the code as an argument", (role, target) => {
    const text = generate(spec(role, target))
      .map((s) => s.text)
      .join("\n");
    expect(text).toContain(PIN);
    expect(text).not.toMatch(/--code[ =]/);
  });

  it("systemd gives the code to one process through env, not argv", () => {
    const [step] = generate(spec("agent", "systemd"));
    expect(step.text).toBe(
      `sudo env WAYHOUSE_ENROLL_CODE=${CODE} wayhouse-agent install --systemd \\\n  --controller-url=https://ctl.example:9901 \\\n  --pin=${PIN} \\\n  -- \\\n  --backends=:25565`,
    );
  });

  it("systemd prompt form carries nothing but the flags", () => {
    const [step] = generate(spec("aggregator", "systemd", { code: null }));
    expect(step.text).toBe(
      `sudo wayhouse-aggregator install --systemd \\\n  --controller-url=https://ctl.example:9901 \\\n  --pin=${PIN}`,
    );
    expect(step.note).toMatch(/paste it there/);
  });

  it("a proxy gets its aggregator and a UI its password credential", () => {
    const proxy = generate(spec("proxy", "systemd", { aggregatorUrl: "https://agg.example:9902" }));
    expect(proxy[0].text).toContain("wayhouse install --systemd");
    expect(proxy[0].text).toContain("--aggregator=https://agg.example:9902");
    const ui = generate(spec("ui", "systemd"));
    expect(ui).toHaveLength(2);
    expect(ui[0].title).toMatch(/password/);
    expect(ui[1].text).toContain("--credential ui-password=/etc/wayhouse/ui/ui-password");
  });

  it("docker enrolls into a local directory, then runs the image with the files read-only", () => {
    const [enroll, run] = generate(spec("proxy", "docker"));
    expect(enroll.text).toContain("mkdir -p ./wayhouse-edge-1");
    expect(enroll.text).toContain(`-e WAYHOUSE_ENROLL_CODE=${CODE}`);
    expect(enroll.text).toContain("ghcr.io/wayhouse-proxy/wayhouse:0.4.0");
    expect(enroll.text).toContain("enroll --data-dir /data");
    expect(run.holdsCode).toBe(false);
    expect(run.text).toContain(`-v "$PWD/wayhouse-edge-1:/data:ro"`);
    expect(run.text).toContain("--cap-drop ALL");
    expect(run.text).toContain("--secret-key-file=/data/secret.keys");
  });

  it("docker prompt form asks on a terminal instead of setting the variable", () => {
    const [enroll] = generate(spec("aggregator", "docker", { code: null }));
    expect(enroll.text).toContain("docker run --rm -it");
    expect(enroll.text).not.toContain("WAYHOUSE_ENROLL_CODE");
  });

  it("the agent container keeps NET_ADMIN and the tun device and nothing else", () => {
    const run = generate(spec("agent", "docker"))[1].text;
    expect(run).toContain("--network host");
    expect(run).toContain("--cap-drop ALL");
    expect(run).toContain("--cap-add NET_ADMIN");
    expect(run).toContain("--device /dev/net/tun");
    expect(run).toContain("--backends=:25565");
  });

  it("compose keeps the code in a secret file and the YAML free of it", () => {
    const steps = generate(spec("aggregator", "compose", { parentUrl: "https://parent.example" }));
    expect(steps[0].text).toBe(`(umask 077; printf '%s\\n' ${CODE} > edge-1.enroll)`);
    const yaml = steps.find((s) => s.language === "yaml")!;
    expect(yaml.title).toBe("compose.edge-1.yml");
    expect(yaml.text).not.toContain(CODE);
    expect(yaml.text).toContain("WAYHOUSE_ENROLL_CODE_FILE: /run/secrets/enroll-code");
    expect(yaml.text).toContain('file: "./edge-1.enroll"');
    expect(yaml.text).toContain("--parent-url=https://parent.example");
    expect(steps.at(-1)!.text).toBe(
      "docker compose -f compose.edge-1.yml run --rm enroll\ndocker compose -f compose.edge-1.yml up -d aggregator\nrm -- edge-1.enroll",
    );
  });

  it("compose prompt form reads the code from the terminal into the file", () => {
    const [save] = generate(spec("proxy", "compose", { code: null }));
    expect(save.text).toBe("(umask 077; cat > edge-1.enroll)");
    expect(save.note).toMatch(/Ctrl-D/);
  });

  it("a compose UI also gets a password secret", () => {
    const steps = generate(spec("ui", "compose"));
    expect(steps.map((s) => s.title)).toContain("Create the login password");
    expect(steps.find((s) => s.language === "yaml")!.text).toContain("ui-password:");
  });

  it("kubernetes ends in a Secret mount and holds no code in the manifest part", () => {
    const steps = generate(spec("proxy", "k8s"));
    expect(steps[1].text).toContain("kubectl -n wayhouse create secret generic wayhouse-edge-1 --from-file=./wayhouse-edge-1");
    const frag = steps.at(-1)!;
    expect(frag.text).toContain("secretName: \"wayhouse-edge-1\"");
    expect(frag.text).toContain("CREDENTIALS_DIRECTORY");
    expect(frag.text).not.toContain(CODE);
  });

  it("an agent on kubernetes is refused with a reason", () => {
    expect(unsupported("agent", "k8s")).toMatch(/init container/);
    expect(unsupported("proxy", "k8s")).toBeNull();
  });
});

describe("quoting and validation", () => {
  it("quotes anything a shell would read as syntax", () => {
    expect(sh("safe-1.2:3")).toBe("safe-1.2:3");
    expect(sh("a b")).toBe("'a b'");
    expect(sh("it's")).toBe(`'it'\\''s'`);
    expect(sh("$(rm -rf /)")).toBe("'$(rm -rf /)'");
  });

  it("accepts what the controller accepts for a name, and nothing it would refuse", () => {
    expect(validName("home-nas.1_a")).toBe(true);
    for (const bad of ["", "a b", "a/b", "../x", "x".repeat(65), "subscribe", "x;y", "é"]) expect(validName(bad)).toBe(false);
  });

  it("checks pins, URLs and backends", () => {
    expect(validPin(PIN)).toBe(true);
    expect(validPin("sha256:abc")).toBe(false);
    expect(validPin(PIN + "00")).toBe(false);
    expect(validUrl("https://ctl.example:9901")).toBe(true);
    expect(validUrl("http://127.0.0.1:9901")).toBe(true);
    expect(validUrl("https://u:p@ctl.example")).toBe(false);
    expect(validUrl("ftp://ctl.example")).toBe(false);
    expect(validUrl("https://a.example/ $(x)")).toBe(false);
    expect(validBackends(":25565,10.0.0.5:7777")).toBe(true);
    expect(validBackends("25565")).toBe(false);
    expect(validBackends("")).toBe(false);
  });
});
