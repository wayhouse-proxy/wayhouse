---
title: "Enrollment: join codes, step-up approvals and scoped tokens"
description: How a new node joins the fleet with a one-time code and receives only the credentials its role needs, why handing out the keyring takes a second signature, and how a first controller bootstraps its own tokens and certificates.
---

A fleet is made of many hosts, and each needs credentials: tokens to talk to the controller, a keyring to open sealed secrets, a tunnel key. The old way was to copy those around by hand. **Enrollment** replaces it with a one-time code. An admin mints a code, a new node redeems it once, and the node gets exactly the files its role needs. This page is about how that is made safe. The commands are in the [Manual](../../manual/index.md) and the generated [Reference](../../reference/index.md), and the tokens it hands out are described in [the controller core](controller-core.md).

## The problem

1. **Credentials must reach a new host without a long lived secret.** Pasting an admin token into a new machine puts the most powerful credential where the least is known.
2. **A code must be worth little if it leaks.** Chat logs and shell histories keep what is typed.
3. **Some credentials are crown jewels.** The cluster keyring opens every sealed secret, and the admin token runs the fleet. Minting a code that carries them must need more than the admin token.
4. **A leaked or compromised component must not be able to mint.** That includes a leaked controller or UI host.
5. **A node must only receive what its role needs,** so a compromised proxy holds a proxy's powers.
6. **The bootstrap problem.** The very first controller has no one to enroll it.

## The code

A code is `<id>.<secret>`. The id is 8 hex characters and finds the record. The secret is 43 characters, the unpadded base64url of **256 random bits**. Only a SHA-256 of the secret is ever stored or replicated, and because the secret has 256 bits no slow password hash is needed. The code type has no `Debug` implementation, since it is a credential until it is redeemed.

The code is shown once at minting. The default lifetime is 900 seconds (15 minutes), and the longest is 7 days. A code can have several uses and a name prefix, which is how a pool of pods in a StatefulSet enroll with one code, each name enrolling once.

## Redeeming

A node redeems with `POST /enroll`, **the only unauthenticated write route** of the controller. Because the code is the credential, the client takes care about where it goes:

- TLS is required except to a loopback host, and the server must be **pinned** by the SHA-256 of its leaf certificate or by a CA file. `http://` to a non loopback host is refused before any connection is made, so a code is never sent to an unverified peer.
- The request body is limited to 1,024 bytes.
- Redeeming is rate limited per source address and per code id, with a window of 60 seconds: 10 attempts per address and 5 per id for redeem, and 30 and 20 for the controller join call, with a bounded table of keys.

A redeem is one entry in the replicated store, applied atomically per Raft index, so a single use code redeems **exactly once** even when nodes race against different controller replicas. The order inside a redeem matters: the use is spent first, then any key is stored, because the other order would let a losing racer overwrite the winner's key. A failure between the two **burns the code**, and the admin mints another. That is the safe side to fail on.

## Role decides the files

The role chosen at minting decides what the answer carries, and the client writes nothing else:

| role       | what it receives                                                                   |
| ---------- | ---------------------------------------------------------------------------------- |
| agent      | its agent token and a fresh tunnel preshared key                                   |
| proxy      | the proxy token, the ingest token if set, and the cluster keyring                  |
| aggregator | the ingest token if set                                                            |
| ui         | the UI token, or the admin token when the controller has no UI token               |
| controller | every token the controller runs with, the keyring, the peer token and a join proof |

The file names are the credential names each binary already resolves, from `$CREDENTIALS_DIRECTORY`, so a directory of them can be given to systemd or mounted as the credentials directory (see [secrets](secrets.md)). On disk the node keeps a directory with mode 0700 holding `enrollment.json`, which has no secret in it, and each credential as its own 0600 file, so a service can be given exactly one of them. `install --systemd` then writes a unit that runs the node with `LoadCredential=` for every secret file, so no token is ever on a command line or in the environment. The origin agent is the exception, because it also writes its WireGuard key in its data directory, and its unit uses `StateDirectory=`.

The origin's tunnel key is made by the controller and sealed under the keyring as `tunnel-psk-<hash of origin>`. The agent receives the plaintext once, in the redeem answer, and proxies fetch the sealed value with their own scoped token (see [the tunnel registry](tunnel-registry.md)). When the controller runs with an admin token, redeeming for an origin also issues it a token of its own.

### Joining a controller

A controller code is the odd one out. Redeeming it only hands over files. The new node then starts with `--ha-join` and **asks to be added** (`POST /enroll/ha-join`), showing the join proof that came with the files, and the cluster runs the member add itself, so the new host never holds an admin token. The node id is chosen when the code is minted and is unique across codes. See [controller HA](controller-ha.md).

## Step up: a second signature for the crown jewels

Minting a code for `proxy`, `ui` or `controller` needs the admin token **and a signed step-up assertion**. A mint made with the UI token needs one for every role, since the UI token is meant to be the weaker credential. The reason is the threat. A stolen admin token, or a compromised UI host, should not be enough to receive the keyring.

The design makes the verifier unable to sign:

- The operator picks a **step-up passphrase**, at least 16 characters. An Ed25519 key is derived from it with **argon2id** and a random 16 byte salt.
- Only the public key and the salt are given to the controller (`--enroll-step-up-pubkey-file`), as one line. The passphrase is never stored. The controller holds nothing that could sign.
- At mint time, whoever holds the passphrase **signs the exact request** with a short expiry and a nonce. The controller verifies it, and a captured assertion cannot mint anything else. An assertion may expire at most 60 seconds ahead, and the signers produce ones that last 30, which is half the verifier's cap so that a signer whose clock runs up to 30 seconds ahead is still accepted.
- A used assertion is remembered for as long as it could still be valid, so it cannot be replayed (the answer is "this step-up assertion was already used").
- Without a configured key, those mints cannot be made at all.

The signer is the UI in a browser flow, or `wayhouse-controller step-up sign` for use with `curl`, which reads the passphrase from a file and never from the command line.

## Scoped tokens

Enrollment hands out scoped tokens, and the controller's scoped authentication is what makes them scoped. Each scoped token unlocks a fixed list of method and path pairs, which is exactly what that component really calls, and nothing else. A proxy token cannot submit config, install plugins or write secrets. The UI token covers the routes the console proxies and not sealed secret reads, tunnel keys, registrations beyond a release, or the machine streams. A scoped token on an allowed request is exchanged for the admin token **once, outside every router**, so the bearer layers inside each module stay untouched, and the list is one place to audit. An origin that enrolled has a token of its own that works for its own name only.

## Bootstrapping the first controller

The first controller has no one to enroll it, so `wayhouse-controller init` does it in one command. It generates the random tokens (admin, proxy, agent, aggregator, ingest), a UI password and a **private certificate authority** with the leaf certificates the control plane servers present, so a bundle is HTTPS from the first start and every client can trust the CA with `--ca-file` or pin the leaf by fingerprint. It writes them where the chosen target reads them: a directory of credentials plus a `tls` directory for systemd, an env file for compose, or Secret manifests for Kubernetes.

The rules are strict:

- Nothing that exists is overwritten. The whole plan is checked first, so a refusal changes nothing.
- Files are created 0600 **from the first byte**.
- **No secret value reaches the terminal.** The summary names files and prints the public fingerprint of the controller certificate, and nothing else. The Kubernetes target, whose output is the secrets, refuses to print to a terminal.

`wayhouse-controller cert` issues or renews a leaf under that CA. A leaf that exists keeps its names plus any new ones, and the key is always new. Nothing is written until every certificate has been made and checked against the CA, and each file is replaced atomically, so a failed run changes nothing and a reader never sees half a file. A certificate this CA did not sign, or one that carries a name `init` never writes, is refused rather than rewritten. Runs are serialised by a lock in the TLS directory. The servers re-read the pair on their own, every 30 seconds.

## What was traded away

- **A burned code on a rare failure.** A crash between spending the use and storing the key costs a code, which is cheap, and avoids the alternative of two nodes sharing a key.
- **The passphrase is a single point of human trust.** Whoever holds it can approve a keyring handout, and losing it means reissuing the public half.
- **The signer has to be online** to mint a proxy, UI or controller code. That is intended.
- **Redemption is one time.** A node that loses its files is re enrolled with a new code, and that replaces its token.
- **The private CA** is simple and self contained, and the CA key sits in the TLS directory until the operator moves it offline.
- **Clock sensitivity.** Assertions are time limited, so the signer and the controller need roughly agreeing clocks.

## Where to look

- `crates/wayhouse-enroll/src/code.rs`: the code format.
- `crates/wayhouse-enroll/src/lib.rs`, `files.rs` and `client.rs`: roles, the on disk layout and the pinned redeem client.
- `crates/wayhouse-enroll/src/step_up.rs`: the passphrase, the key and the assertion.
- `crates/wayhouse-enroll/src/install.rs` and `cli.rs`: the node commands and the systemd unit.
- `crates/wayhouse-controller/src/enroll.rs`: minting, redeeming, limits and the join.
- `crates/wayhouse-controller/src/scoped_auth.rs`: the token scopes.
- `crates/wayhouse-controller/src/init.rs`, `cert_cli.rs` and `step_up_cli.rs`: bootstrap, certificates and the signer.
