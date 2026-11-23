# Sending players to the nearest healthy node

Several Wayhouse nodes only help latency and failover if clients are sent to the nearest healthy
one. This page shows what works today (DNS names and a provider you already use), what DNS can and
cannot do, and where anycast fits. Tracking issue:
[#796](https://github.com/wayhouse-proxy/wayhouse/issues/796).

Status: **documentation only.** Wayhouse does not change DNS records yet. A controller-driven
steering plugin is planned (see [Planned](#planned)). Statements tagged **unverified** come from
memory or could not be confirmed against a provider's own page; check them before you rely on them.
Provider facts below were read on 2026-11-23 and providers change prices and limits, so look at the
linked pages.

## Spread the backends too

Steering clients to a nearby node helps only if the path from that node to the game server is
good. A player in Sydney who reaches a Sydney node whose backend sits in Frankfurt gets a worse path
than before, plus one extra hop.

Steering pays off when:

- the game servers exist in several regions, each region's nodes route to the backends of that
  region (separate pools per region);
- or the nodes reach their backends over a private backbone or the [tunnel](11-backend-transport.md)
  that beats the public path;
- or the aim is to absorb traffic near the source (see [Security and DDoS](07-security-ddos.md)).

Wayhouse has no "prefer the nearest backend" balancing today: a node picks among the backends its
pool lists. Give each region's nodes a pool with that region's backends.

## What works today: one name per region

The simplest setup needs no new feature. Publish one hostname per region and let players pick, or
let your launcher or server list pick:

```
eu.play.example.com    A  203.0.113.10, 203.0.113.11   (EU nodes)
us.play.example.com    A  198.51.100.20, 198.51.100.21 (US nodes)
play.example.com       CNAME  eu.play.example.com       (default region)
```

Wayhouse already routes by hostname and SNI, so each node serves its names with a normal route (see
[Routing](03-routing.md)). Several A records for one name are plain round robin: there is no health
awareness, and a client that picks a dead node's address stays failed until it retries.

For automatic nearest-region answers use your DNS provider's geographic or latency routing on the
global name, with the per-region names as the targets:

- **Amazon Route 53** has latency, geolocation, geoproximity, IP-based, weighted, failover and
  multivalue routing as attributes of ordinary records
  ([routing policies](https://docs.aws.amazon.com/Route53/latest/DeveloperGuide/routing-policy.html)).
  Geolocation and geoproximity queries are billed at $0.70 per million, latency at $0.60, standard
  at $0.40 ([pricing](https://aws.amazon.com/route53/pricing/)).
- **Cloudflare Load Balancing** offers geo, dynamic, proximity, random and failover steering. Without
  the steering add-on only random and failover are available
  ([steering policies](https://developers.cloudflare.com/load-balancing/understand-basics/traffic-steering/steering-policies/)).
  The product is a paid add-on, listed as "starting at $5/mo"
  ([plans](https://www.cloudflare.com/plans/)); other fees were not confirmed. Geo needs pools and
  monitors (provider objects) and not just records.
- Other providers (NS1, Azure Traffic Manager, Google Cloud DNS routing policies, bunny.net,
  ClouDNS) were **not researched**.

## What DNS steering can and cannot do

**It is connect-time routing.** A client resolves the name when it connects, picks an address and
keeps it for the session. A DNS change never moves a live session. When a node dies, its sessions
break, clients reconnect, and the new lookup should return a healthy node. Wayhouse nodes share no
session state (ADR 4 in [Technology choices](09-technology-choices.md)), so a moved player is a
reconnect to a fresh session.

**Failover time is a sum:** health detection (probe interval times failures), how fast your DNS
records are changed, the record TTL, and client or resolver caching. With a TTL of 30 to 60 seconds
and automatic record changes, expect roughly one to three minutes for players who reconnect. Players
who never resolve again do not benefit at all.

**Client caching:**

- Java caches lookups according to `networkaddress.cache.ttl`. Its documented default is -1 (cache
  forever) when a security manager is installed and "implementation-specific" otherwise; failed
  lookups are cached 10 seconds by default
  ([Java networking properties](https://docs.oracle.com/en/java/javase/21/docs/api/java.base/java/net/doc-files/net-properties.html)).
  A 30 second default without a security manager is **unverified**.
- Many UDP games take an IP and port from a server browser or a launcher file and never do DNS. A
  DNS-based scheme cannot reach them. For those games publish a list of per-region addresses, or use
  anycast.
- Resolvers can keep records longer than the TTL. **Unverified**: how common this is today.

**Geo accuracy depends on the resolver.** An authoritative server sees the resolver's address, not
the player's, unless the resolver passes the EDNS Client Subnet (ECS). Google Public DNS sends ECS
only to zones whose name servers answer it correctly
([ECS guidelines](https://developers.google.com/speed/public-dns/docs/ecs)). Cloudflare's 1.1.1.1
does not send ECS ([FAQ](https://developers.cloudflare.com/1.1.1.1/faq/)). Players on such resolvers
are placed by the resolver's location, which is usually near them but not exact.

**Health checks and UDP.** The Route 53 health check types are HTTP, HTTPS, TCP, calculated,
CloudWatch alarm and routing control; UDP is not mentioned
([types](https://docs.aws.amazon.com/Route53/latest/DeveloperGuide/health-checks-types.html)), so
treat UDP as unsupported. Cloudflare monitors include TCP, ICMP ping and "UDP-ICMP"
([monitors](https://developers.cloudflare.com/load-balancing/monitors/)); the minimum interval and
plan limits were not confirmed. For UDP-only nodes, health checks from the provider may not tell you
whether the game port works.

## SRV records where games support them

An SRV record gives host, port, priority and weight, so one name can point at several nodes and
ports:

```
_minecraft._tcp.play.example.com  SRV  10 60 25565 eu.play.example.com.
_minecraft._tcp.play.example.com  SRV  10 40 25565 us.play.example.com.
```

- Minecraft Java Edition looks up `_minecraft._tcp` through JNDI
  ([JDK-8272997](https://bugs.openjdk.org/browse/JDK-8272997)). A Mojang bug reports that priority is
  ignored and targets are picked at random, and it was still unresolved when read
  ([MC-49826](https://bugs-legacy.mojang.com/browse/MC-49826)). Do not use SRV priority as your
  failover.
- Minecraft Bedrock Edition and most other games: SRV support is **unverified** and usually absent.
  Check the game before relying on it.
- Cloudflare's DNS load balancer handles SRV only on Enterprise plans (A, AAAA and CNAME on others)
  ([DNS records](https://developers.cloudflare.com/load-balancing/load-balancers/dns-records/)).

## Anycast (bring your own)

With anycast every node announces the same address and routing picks the nearest one. There is no
DNS caching problem and failover is a route withdrawal, which is why it suits UDP games best. It
needs your own AS number and address block, or a provider that sells it. Wayhouse does not build
anycast; operate it outside and put nodes behind it.

- Requirements and caveats are in [Operations](06-operations-observability.md#fronting-layer-anycast-vs-l4-load-balancer):
  flow-consistent ECMP, and `transparent: true` needs the reply path through the same node. Prefer
  PROXY protocol when the fronting layer can rehash a flow.
- Before you withdraw a route for maintenance, take the node out with `POST /admin/drain` so
  `readyz` turns false.
- **AWS Global Accelerator** is a managed anycast that supports UDP. It gives two static IPv4
  addresses per accelerator, and its endpoints must be a Network Load Balancer, Application Load
  Balancer, EC2 instance or Elastic IP; on-premises machines are reached through a load balancer
  ([how it works](https://docs.aws.amazon.com/global-accelerator/latest/dg/introduction-how-it-works.html),
  [FAQ](https://aws.amazon.com/global-accelerator/faqs/)). UDP connections time out after 30
  seconds idle. Cost is $0.025 per hour per accelerator (about $18 a month) plus a per-GB
  data-transfer premium ([pricing](https://aws.amazon.com/global-accelerator/pricing/)).
- **Cloudflare Spectrum** proxies UDP only as a paid add-on on Enterprise
  ([protocols per plan](https://developers.cloudflare.com/spectrum/protocols-per-plan/)).
- Vultr BYOIP and Fly.io were suggested as anycast sources and **not checked**.

## UDP and affinity across nodes

- A returning player on another node opens a new proxy session. A backend that binds a session to
  the source address needs PROXY protocol, so the real client address survives the extra hop.
- A `consistent_hash` pool hashes the client address to choose a backend
  ([Routing](03-routing.md)). Two nodes with the same healthy backend set should choose the same
  backend for the same client without shared state. That is a property of the algorithm; it has
  **not been tested across nodes under the tunnel**.
- Keep UDP idle timeouts short (tens of seconds), as [Operations](06-operations-observability.md)
  advises, so a moved client re-establishes quickly.

## Planned

A steering plugin is planned (tracked in
[#796](https://github.com/wayhouse-proxy/wayhouse/issues/796)). The idea, none of it built:

- The controller leader decides which nodes are healthy, from peer reports and drain state, so UDP
  nodes are judged from the inside rather than by provider probes.
- It writes plain A and AAAA record sets per region, with optional Route 53 latency or geolocation
  attributes, through the same plugin mechanism as the ACME DNS-01 provider
  ([`acme.md`](acme.md)).
- It keeps the last records when the controller is unavailable, never removes the last healthy node
  of a region, and holds a recovered node out for a while to damp flapping.
- Open: nodes reporting their own health to the controller, backend locality, and whether the plugin
  needs a new read-only capability and an ABI bump.

A DNS write token can redirect every player. When this exists, scope the token to the zone and give
it to the plugin only through the sealed secret store ([`plugins.md`](plugins.md)).
