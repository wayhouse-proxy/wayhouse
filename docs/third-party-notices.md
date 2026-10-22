# Third-party notices

Wayhouse itself is `MIT OR Apache-2.0`. This page records the third-party software that ships
inside Wayhouse's own images and the decisions behind it. Rust dependencies are audited in CI (`cargo audit`); this page is about what is
redistributed as a binary.

## Caddy (`wayhouse-caddy` image)

| Item         | Value                                                                                               |
| ------------ | --------------------------------------------------------------------------------------------------- |
| Software     | [Caddy](https://github.com/caddyserver/caddy) 2.11.7, stock build, unmodified                       |
| License      | Apache-2.0                                                                                          |
| Pinned in    | `deploy/Dockerfile` (version and digest), checked by `deploy/check-caddy-image.sh`                  |
| In the image | `/licenses/LICENSE-APACHE` and `/licenses/NOTICE` ([`deploy/caddy/NOTICE`](../deploy/caddy/NOTICE)) |

Apache-2.0 allows redistribution. What we do to meet its terms: ship the license text, keep the
attribution in the NOTICE, and state changes. We change no Caddy source; the binary is copied
from the upstream image only to drop its file capability. Caddy's own Go dependencies (BSD, MIT
and similar) are compiled in and are covered by the upstream project's license files; the image
does not list them one by one. Upstream's module list is `caddy list-modules --versions`.

### Modules

There are none. The image is stock Caddy; the ACME and DNS-01 work is done by the controller,
not by Caddy plugins. If a module is added later (an `xcaddy build --with <module>` step in
the Dockerfile), it needs, before it may ship:

1. a license that allows redistribution, confirmed from the module's repository and recorded in
   [`deploy/caddy/MODULES`](../deploy/caddy/MODULES);
2. an entry in `deploy/caddy/NOTICE` with its version;
3. a green `deploy/check-caddy-modules.sh`, which `make deploy-lint` runs: it fails when a
   `--with` module has no recognised license or is missing from the NOTICE.

A module without a license grant (as `caddy-dns/route53` showed when checked in October 2026)
stays out of the image. `caddy-dns/cloudflare` is Apache-2.0.

### Trademark

Checked on 2026-10-22. caddyserver.com publishes no separate trademark policy page; the
statements that apply are in the upstream repository:

- The README says: "The name "Caddy" is trademarked. The name of the software is "Caddy", not
  "Caddy Server" or "CaddyServer". Please call it "Caddy" or, if you wish to clarify, "the Caddy
  web server". Caddy is a registered trademark of Stack Holdings GmbH."
- Apache-2.0 section 6 grants no right to use trade names or product names, except for
  reasonable and customary use in describing the origin of the work and reproducing the NOTICE.

Our position:

- We ship stock, unmodified Caddy, so calling it "Caddy" and saying where it came from is the
  descriptive use that section 6 allows.
- Our image is called `wayhouse-caddy`, our product name first. The docs say "Caddy" when they
  mean the upstream software, and describe our image as "Wayhouse's packaging of Caddy".
- We do not brand our build as "Caddy", do not use Caddy's logo, and do not suggest that the
  Caddy project or Stack Holdings GmbH endorses Wayhouse.
- If we ever patch Caddy, the image must stop calling itself Caddy without a closer reading of
  upstream's terms.
