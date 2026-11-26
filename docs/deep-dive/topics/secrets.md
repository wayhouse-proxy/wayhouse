---
title: "Secrets: the sealed store, references and redaction"
description: How wayhouse keeps tokens and keys out of config files, logs and command lines, how a secret is sealed and bound to its name, and why a proxy never receives plaintext from the controller.
---

A proxy needs a few secrets: the admin bearer token, the gossip pre-shared key, and for plugins the credentials they use to call out. Each is a place where a leak turns into an incident, and each has several natural ways to leak: a config file in git, a process listing, a log line, a backup of a database. This page is about the rules that keep them out of those places. The options are in the [Manual](../../manual/index.md) and the generated [Reference](../../reference/index.md).

## The problem

1. **Config is shared, secrets must not be.** A config revision is stored, diffed, reviewed and pasted into tickets. A token inside it travels everywhere.
2. **The command line is public.** Arguments show up in `ps`, in `/proc/<pid>/cmdline` and in `docker inspect`.
3. **Storage is not the trust boundary.** A controller's database, its snapshots and its replication log are copied around. They must be useless to someone who only has them.
4. **The server must not be able to lie.** A controller that hands out secrets could be asked, by an attacker holding it, to hand back an older value, such as a rotated token that was already revoked.
5. **Plugins act on the operator's behalf.** A module that calls an external API needs a credential, and it must never be able to read it or print it.

## Four ways in, in a fixed order

`wayhouse-http`'s `secret_arg` module decides where a binary gets a token or password. For a secret called `name` the lookup order is:

1. the file named by `--<name>-file`,
2. `$CREDENTIALS_DIRECTORY/<name>`, which is what systemd's `LoadCredential=` provides,
3. the environment variable `WAYHOUSE_<NAME>`, with `-` turned into `_`.

A trailing newline in a file is dropped. An empty secret is an error and never "no secret": a mis-mounted file must not quietly turn authentication off. There is deliberately no `--token <value>` form for the same reason as the first rule above.

## References in config

A config value that holds a secret can be plaintext or a reference, `psk: { secret: gossip-psk }`. The [config crate](config-schema.md) parses the reference and nothing more. A `SecretResolver` supplied by the binary turns the name into text, and a reference that resolves to nothing is an error, never an empty token. The messages name the secret and what is missing: no store is configured, the name is not in the store, the value cannot be opened, or it is empty. None contains a value.

Plaintext still works and is allowed, with a warning, so a small deployment can start with one file. Once a revision is replaced, the controller scrubs the plaintext from the stored copy (see redaction in the config page).

Secret names are 1 to 63 characters of `a-z`, `0-9` and `-`, not starting or ending with `-`.

## Sealing at rest

The sealed store lives in the `wayhouse-secrets` crate and is shared by the controller, the proxy and the agent. The design is conventional on purpose.

- **One cluster keyring.** The same key file is provisioned to every node out of band (`--plugin-secret-key-file`, or the environment variable `WAYHOUSE_PLUGIN_SECRET_KEY`). The file holds one base64 key per line, each exactly 32 bytes, with blank lines and `#` comments allowed. **The last key is the active one**, and the earlier ones stay so old secrets can still be opened.
- **XChaCha20-Poly1305** with a random 192-bit nonce. The long nonce is what makes random nonces safe for a store that is written to over a long time.
- **Bound to a place.** The associated data is `scope`, `name` and the key id, joined by zero bytes. For a plugin secret the scope is the install id and the name is the slot. For a cluster secret (the admin token, the gossip key) the scope is a fixed one that no install id can equal, because install ids are hex. A ciphertext copied to another name or scope therefore fails to open, which turns "someone with write access to the database" into "someone who can delete secrets, but not swap them".
- **Ciphertext only in storage.** The log, the snapshots and the database hold the `Sealed` form: key id, nonce, ciphertext and, in the current format, a revision. The keyring never leaves the process.
- **The key file must be private.** A key file that group or others can read is refused with "chmod 600 it".
- **Bounds.** A secret is at least 8 bytes (scrubbing a shorter value would mangle ordinary text and protect nothing) and at most 4,096.
- **Key ids** are a hash of the key with a fixed label, truncated, so logs and error messages can say which key is missing without printing any key material.

The plaintext type, `SecretValue`, is zeroed when dropped, prints as `<redacted>` for both `Debug` and `Display`, and exposes its bytes through one method that is called only where the value is written into a request.

### Rotation

The key file is polled every 5 seconds for a change, so adding a key at the end of the file rotates the active key without a restart, and the old keys remain so that secrets sealed under them can still be opened.

## Rollback protection

A sealed secret in the current format carries a **revision**, and the revision is part of what the authentication tag covers, so a blob cannot be relabelled as a newer one. A reader that opens every blob it is served through a `RevFloor` remembers the highest revision it has seen for each name and refuses an older one with a rollback error. A blob without a revision (the legacy format) is refused too, with a message telling the operator to have the controller rewrap it.

So a hostile or confused server cannot walk a proxy back to a rotated token or an old gossip key. The honest limit is written in the source: the floor lives in memory, so after a restart it starts empty, and a hostile server can roll a reader back once per restart.

## Where a proxy gets its secrets

Two ways, chosen by how the proxy is configured.

- **From the controller.** The proxy fetches each referenced secret's **ciphertext** and opens it with its own copy of the key. The controller never sends plaintext, and a proxy without the key cannot read what it is sent. If a secret the config names cannot be opened, startup stops with the reason, rather than running with an empty token.
- **From a local store.** A proxy on a plain file config has no controller to ask. It keeps a JSON file of `name -> sealed value` (`--secrets-file`), sealed under its own key file with the same name bound scope the controller uses, so a value moved to another name or opened with another key fails. The file holds ciphertext only and must be owner only. It is written to a temp file named after the store file plus the process id, opened with `O_EXCL` so a leftover or planted file (wrong mode, a symlink) is an error and never the store, and then renamed over the old one. One shot commands manage it: `--secret-set NAME` reads the value from standard input, so it never touches argv or shell history, `--secret-delete NAME` removes one, and `--secret-list` prints names and never values.

## Plugins and the scrubber

A plugin declares secret slots, and the operator fills them. When a plugin makes a call, the host expands a slot into the request, and the value lives only for that host call. It is never passed to the guest and never printed.

What the destination echoes back is the other half of the problem. A server may repeat a header, a URL or an error body that contains the credential in some encoding. The host's `Scrubber` is built from the secrets of the call and removes every form a destination could echo: the raw text, base64 in the standard and URL safe alphabets with and without padding, and percent encoding, including the lower case form of each escape (`%2F` and `%2f`). What replaces them is `[redacted]`. A secret below the 8 byte minimum is skipped, for the same reason as the store's minimum.

## What was traded away

- **One shared keyring.** Every node holds the same key, so a stolen node means a stolen keyring. The export of the key file is something only enrollment does.
- **Revision floor in memory.** A restart resets it, as noted above.
- **Sealing protects storage, not the running process.** The store protects secrets at rest and in replication. A process that holds the key and the ciphertext can open it.
- **Scrubbing is best effort.** A destination that echoes a secret transformed in some other way, such as hashed or split across lines, is not caught. The scrubber covers the encodings that are common, not every possible one.
- **Plaintext is allowed.** The warning is the only pressure, because refusing plaintext would make a first run harder than it needs to be.

## Where to look

- `crates/wayhouse-secrets/src/lib.rs`: the keyring, sealing, `RevFloor` and the name rules.
- `crates/wayhouse-config/src/secret.rs`: `SecretOr`, `SecretResolver` and the error messages.
- `crates/wayhouse/src/secrets.rs`: the proxy resolving referenced secrets from the controller.
- `crates/wayhouse/src/secret_store.rs`: the proxy local store and the one shot commands.
- `crates/wayhouse-http/src/secret_arg.rs`: file, credentials directory and environment lookup.
- `crates/wayhouse-plugin-host/src/secret.rs`: `SecretSource` and the `Scrubber`.
