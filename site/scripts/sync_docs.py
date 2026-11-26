#!/usr/bin/env python3
"""Builds the Starlight content tree from ../docs/ (the Markdown stays the source of truth).

Writes (all git-ignored): src/content/docs/** (pages with generated frontmatter), public/img/**
(the diagrams), public/docs/*.html (redirects from the old mdBook URLs), and copies landing.mdx.

What it does to each page: takes the title from the first `# ` heading (numbering such as
"02 - " dropped), turns GitHub alerts (`> [!NOTE]`) into Starlight asides, and rewrites every
relative link: to another page -> that page's new URL, to an image -> /img/..., to anything else
in the repository (crates/, deploy/, unpublished docs) -> its GitHub URL. Stdlib only.
"""
import json
import os
import re
import shutil
import sys

SITE = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
REPO = os.path.dirname(SITE)
DOCS = os.path.join(REPO, "docs")
OUT = os.path.join(SITE, "src", "content", "docs")
PUBLIC = os.path.join(SITE, "public")
GITHUB = "https://github.com/wayhouse-proxy/wayhouse"

SKIP_DIRS = {"superpowers", "design-system", "img"}  # unpublished or copied separately
SKIP_FILES = {"README.md", "SUMMARY.md"}  # the old mdBook intro and table of contents
CHAPTER = re.compile(r"^(\d\d)-(.+)\.md$")
# Top-level pages that are reference material rather than manual pages.
REFERENCE = {"third-party-notices"}
# Where the older top-level guides sit in the Manual (everything else lands in manual/).
MANUAL_SECTION = {
    "upgrading": "operate", "migrating": "operate", "acme": "certificates", "plugins": "extend",
    "sniffers": "extend", "web-proxy": "web", "global-steering": "routing", "ui-data": "ui",
    "ui-form-layout": "ui",
}
# Top-level pages that explain how something is built.
DEEP_DIVE = {"latency-ledger"}


def dest_of(rel: str):
    """Maps a path under docs/ to (url path without slashes, sidebar order or None)."""
    parts = rel.split("/")
    if len(parts) > 1:  # docs/manual/x.md, docs/deep-dive/x.md, ...: already where it belongs
        return rel[: -len(".md")], None
    m = CHAPTER.match(rel)
    if m:
        return "deep-dive/design/" + m.group(2), int(m.group(1)) + 1  # the overview page is 0
    name = rel[: -len(".md")].replace(".", "")  # Starlight slugs drop dots: migrating-0.2 -> migrating-02
    section = "reference/" if name in REFERENCE else "deep-dive/" if name in DEEP_DIVE else \
        "manual/" + (MANUAL_SECTION.get(name, "") + "/" if name in MANUAL_SECTION else "")
    return section + name, None


MISSING = []


def collect():
    pages = {}
    for dirpath, dirs, names in os.walk(DOCS):
        dirs[:] = sorted(d for d in dirs if not (dirpath == DOCS and d in SKIP_DIRS))
        for name in sorted(names):
            if not name.endswith(".md") or (dirpath == DOCS and name in SKIP_FILES):
                continue
            rel = os.path.relpath(os.path.join(dirpath, name), DOCS).replace(os.sep, "/")
            pages[rel] = dest_of(rel)
    return pages


EXTERNAL = re.compile(r"^(?:[a-z][a-z0-9+.-]*:|#|/)", re.I)


def make_rewriter(src_rel: str, pages: dict):
    src_dir = os.path.dirname(src_rel)

    def rewrite(href: str) -> str:
        if EXTERNAL.match(href) or not href:
            return href
        path, _, frag = href.partition("#")
        frag = "#" + frag if frag else ""
        if not path:
            return href
        target = os.path.normpath(os.path.join(src_dir, path)).replace(os.sep, "/")
        if target in pages:
            url = pages[target][0]
            url = url[: -len("/index")] if url.endswith("/index") else url
            return "/" + url + "/" + frag
        if target == "README.md":  # the old docs index
            return "/" + frag
        if target.startswith("img/"):
            return "/" + target
        repo_path = os.path.normpath(os.path.join("docs", target)).replace(os.sep, "/")
        top = target.split("/")[0]
        if not target.startswith("..") and top not in SKIP_DIRS and target.endswith(".md"):
            MISSING.append("%s: link to a docs page that does not exist: %s" % (src_rel, href))
        return GITHUB + "/blob/main/" + repo_path + frag

    return rewrite


LINK = re.compile(r"(\]\()([^)\s]+)(\))")
ATTR = re.compile(r'\b(href|src|srcset)="([^"]*)"')
INLINE_CODE = re.compile(r"(`[^`]*`)")
ALERT = re.compile(r"^>\s*\[!(NOTE|TIP|IMPORTANT|WARNING|CAUTION)\]\s*$", re.I)
ASIDE = {"NOTE": "note", "TIP": "tip", "IMPORTANT": "note", "WARNING": "caution", "CAUTION": "danger"}


def rewrite_line(line: str, rewrite) -> str:
    parts = INLINE_CODE.split(line)
    for k in range(0, len(parts), 2):  # even parts are outside inline code
        parts[k] = LINK.sub(lambda m: m.group(1) + rewrite(m.group(2)) + m.group(3), parts[k])
        parts[k] = ATTR.sub(lambda m: '%s="%s"' % (m.group(1), rewrite(m.group(2))), parts[k])
    return "".join(parts)


def transform(text: str, rewrite) -> str:
    out, fence, i = [], None, 0
    lines = text.split("\n")
    while i < len(lines):
        line = lines[i]
        stripped = line.lstrip()
        if fence:
            out.append(line)
            if stripped.startswith(fence):
                fence = None
            i += 1
            continue
        if stripped.startswith("```") or stripped.startswith("~~~"):
            fence = stripped[:3]
            out.append(line)
            i += 1
            continue
        m = ALERT.match(line)
        if m:
            out.append(":::" + ASIDE[m.group(1).upper()])
            i += 1
            while i < len(lines) and lines[i].startswith(">"):
                out.append(rewrite_line(re.sub(r"^>\s?", "", lines[i]), rewrite))
                i += 1
            out.append(":::")
            continue
        out.append(rewrite_line(line, rewrite))
        i += 1
    return "\n".join(out)


FRONT = re.compile(r"\A---\n(.*?\n)---\n", re.S)


def frontmatter_and_body(text: str, rel: str, order):
    """Returns (frontmatter lines, body). Pages that already carry a title keep their own."""
    m = FRONT.match(text)
    if m:
        fm = m.group(1).rstrip("\n").split("\n")
        body = text[m.end():]
        has_title = any(l.startswith("title:") for l in fm)
    else:
        fm, body, has_title = [], text, False
    if not has_title:
        h = re.search(r"^# +(.+?)\s*$", body, re.M)
        if not h:
            sys.exit("%s: no frontmatter title and no '# ' heading" % rel)
        title = re.sub(r"^\d+\s*[–-]\s*", "", h.group(1))
        body = body[: h.start()] + body[h.end():].lstrip("\n")
        fm.insert(0, "title: " + json.dumps(title, ensure_ascii=False))
    if order is not None and not any(l.startswith("sidebar:") for l in fm):
        fm += ["sidebar:", "  order: %d" % order]
    if not any(l.startswith("editUrl:") for l in fm):
        fm.append("editUrl: %s/edit/main/docs/%s" % (GITHUB, rel))
    return fm, body


def redirect_page(url: str) -> str:
    return (
        '<!doctype html><meta charset="utf-8"><title>Moved</title>'
        '<link rel="canonical" href="%(u)s"><meta http-equiv="refresh" content="0; url=%(u)s">'
        '<script>location.replace("%(u)s" + location.hash)</script>'
        '<a href="%(u)s">This page moved</a>\n' % {"u": url}
    )


def main() -> int:
    for d in (OUT, os.path.join(PUBLIC, "docs"), os.path.join(PUBLIC, "img")):
        shutil.rmtree(d, ignore_errors=True)
    pages = collect()
    for rel, (dest, order) in pages.items():
        with open(os.path.join(DOCS, rel), encoding="utf-8") as f:
            fm, body = frontmatter_and_body(f.read(), rel, order)
        body = transform(body, make_rewriter(rel, pages))
        target = os.path.join(OUT, dest + ".md")
        os.makedirs(os.path.dirname(target), exist_ok=True)
        with open(target, "w", encoding="utf-8") as f:
            f.write("---\n" + "\n".join(fm) + "\n---\n\n" + body.lstrip("\n"))
        if "/" not in rel:  # an old mdBook URL: /docs/<name>.html
            url = "/" + (dest[: -len("/index")] if dest.endswith("/index") else dest) + "/"
            os.makedirs(os.path.join(PUBLIC, "docs"), exist_ok=True)
            with open(os.path.join(PUBLIC, "docs", rel[:-3] + ".html"), "w") as f:
                f.write(redirect_page(url))
    os.makedirs(os.path.join(PUBLIC, "docs"), exist_ok=True)
    with open(os.path.join(PUBLIC, "docs", "index.html"), "w") as f:  # old book entry point
        f.write(redirect_page("/manual/"))
    img = os.path.join(DOCS, "img")
    if os.path.isdir(img):
        shutil.copytree(img, os.path.join(PUBLIC, "img"))
    shutil.copy(os.path.join(SITE, "landing.mdx"), os.path.join(OUT, "index.mdx"))
    sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
    import reference_pages
    for dest, render in reference_pages.PAGES.items():
        with open(os.path.join(OUT, dest + ".md"), "w", encoding="utf-8") as f:
            f.write(render())
    if MISSING:
        sys.exit("\n".join(sorted(set(MISSING))))
    print("synced %d pages" % len(pages))
    return 0


if __name__ == "__main__":
    sys.exit(main())
