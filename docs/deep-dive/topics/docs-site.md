---
title: The documentation site
description: How the docs are written, generated, checked and published.
---

The documentation is Markdown in `docs/`, and it is published as a website with three
sections: a **Manual** for people who run wayhouse, a **Deep dive** for people who want to know
how it is built, and a **Reference** of lookup tables. This page explains how that works, and
why it is built the way it is. It is also the first Deep dive page, so it doubles as an example
of the style the others follow.

## The problem

Three things pull in different directions.

1. **The Markdown has to stay plain Markdown.** It is read on GitHub, in editors and by coding
   agents as often as on the site. So pages link to each other with ordinary relative links
   (`../05-configuration.md`), not site-specific URLs, and they carry no site-only syntax.
2. **The site has a structure the files do not have.** The old layout was thirteen numbered
   design chapters plus a pile of guides. The site needs sections, a sidebar order and
   readable titles, and several authors change `docs/` at the same time, so moving files
   around to suit the generator would turn every parallel change into a merge conflict.
3. **Parts of the docs must never be written by hand.** The list of config keys, flags,
   metrics and endpoints is large, changes with every release and is easy to get subtly
   wrong. A hand-maintained list drifts within a few releases.

## The design

```text
docs/**/*.md ─┐
              ├─► site/scripts/sync_docs.py ─► site/src/content/docs/ ─► Astro + Starlight ─► site/dist
crates/**    ─┘   (reference.py reads source)   (generated, git-ignored)
```

**Authoring stays in `docs/`.** `sync_docs.py` copies every page into the content folder of an
[Astro](https://astro.build) project that uses [Starlight](https://starlight.astro.build),
and changes it on the way:

- The title comes from the page's first `# ` heading, with chapter numbers (`02 – `) dropped.
  A page that already has frontmatter keeps it.
- GitHub alerts (`> [!NOTE]`) become Starlight asides.
- Every relative link is rewritten: to another page, to that page's new URL; to an image, to
  its site path; to anything else in the repository, such as `crates/` or `deploy/`, to its
  GitHub URL. A link to a docs page that does not exist fails the build instead of quietly
  turning into a dead GitHub link.
- A small table decides where the older top-level pages land. `03-routing.md` becomes
  `deep-dive/design/routing`, `acme.md` becomes `manual/certificates/acme`. Because the table
  lives in the script, nobody has to move a file to change the site structure.

**Old URLs keep working.** The script also writes a tiny redirect page for every old
`/docs/<name>.html` address, so links from issues, chat and search engines survive the move.

**Search and styling** come with Starlight: full-text search is built at deploy time and runs
in the browser, and light and dark themes follow the visitor's system.

## The generated Reference

`reference.py` reads the source tree directly and never compiles anything, which keeps the
site build to seconds and lets it run in a plain docs job:

| Reference page        | Where it comes from                                                                                                 |
| --------------------- | ------------------------------------------------------------------------------------------------------------------- |
| Config keys           | The checked-in JSON Schema of the config, which the config crate keeps in sync with its types through a drift test. |
| Command-line flags    | The clap argument structs of every binary, including doc comments, defaults and subcommands.                        |
| Metrics               | The `pub const NAME: &str = "wayhouse_..."` definitions and the doc comment above each.                             |
| Feature flags         | The runtime flag registry, and the `[features]` tables of the crates.                                               |
| HTTP API endpoints    | The `.route("/path", get(..).post(..))` calls of the axum routers.                                                  |
| UI screens            | The route table of the frontend.                                                                                    |
| Environment variables | `env =` attributes, `env::var` calls and `WAYHOUSE_*` names in the source.                                          |

Extracting from source text is less exact than asking the compiler, and it fails in a quiet
way: if a refactor changes how routes are declared, the extractor finds fewer of them and the
page just gets shorter. That is why the coverage check has a **minimum count per kind**.

## Making sure nothing is missing

"Document every feature" is only achievable if it is enforced, so CI enforces it twice.

**Options.** `check_coverage.py` takes every config key, flag, metric, runtime flag, endpoint,
screen and environment variable from the extractors and checks that the hand-written pages
(not the generated Reference, which lists everything by construction) mention it. Known gaps
sit in `docs/coverage-allowlist.txt`. The list works as a ratchet: a new undocumented item
fails the check, and so does a listed item that has since been documented, so the file only
shrinks and doubles as the Manual's to-do list. The check runs in the `docs` job of the main CI
workflow, so a gap cannot merge, and it is deliberately not part of the Pages workflow, so a gap
can never block a site deploy.

**Mechanisms.** Options are only half of a feature. Internal mechanisms such as caching, gossip,
retry and backoff, signing and the release pipeline have no flag to grep for. `docs/deep-dive/topics.tsv`
maps every source file to the Deep dive topic that explains it, and `check_deep_dive.py` fails
when a file belongs to no topic, so new code cannot appear without a place for its explanation.
A topic is `todo` until its page exists.

## Publishing

`pages.yml` builds the site with a pinned Node version and `npm ci` against a committed
lockfile, so a build is reproducible, and every action in the workflow is pinned by commit.
Pull requests only build; a push to `main` that touches the docs, the site or the sources the
Reference reads deploys to GitHub Pages at `wayhouse-proxy.dev`. The site is deliberately not
part of the merge gate: a broken site must not stop a code fix from merging. Starlight's link
validator does fail the site build on any broken internal link or anchor, which is the check
the old toolchain did in a post-processing script.

## Trade-offs

- **A generation step instead of moving files.** The script is one more thing to maintain, but
  it keeps `docs/` readable on GitHub and out of everyone's way. If the site structure stops
  changing, the table in the script could be replaced by moving the files for real.
- **Parsing source instead of compiling.** Fast and dependency-free, but approximate. The
  floors and the allowlist ratchet turn "approximate" into "fails loudly when it breaks".
- **Starlight instead of mdBook.** The old book needed no JavaScript toolchain, but it had no
  way to group content into sections, no good search and no component for the landing page.
  Starlight adds a Node toolchain to a Rust repository in exchange; Dependabot watches
  `site/` to keep it current.
