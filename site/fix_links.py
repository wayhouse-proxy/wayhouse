#!/usr/bin/env python3
"""Post-build step for the docs site: repo-relative links become GitHub links, then every
relative link in the built book must resolve to a file (exit 1 otherwise).

The Markdown in docs/ links to files outside docs/ (`../crates/...`, `../README.md`) and to the
unpublished docs/superpowers/ records. Those work on GitHub but 404 on the site, so they are
pointed at the repository on `main` instead. Usage: fix_links.py <built-book-dir>. Stdlib only.
"""
import os
import re
import sys

REPO = "https://github.com/wayhouse-proxy/wayhouse/blob/main/"
HREF = re.compile(r'href="([^"]*)"')


def rewrite(page_dir: str, root: str, href: str) -> str:
    """Returns the repo URL for a link that leaves the book, else the href unchanged."""
    if re.match(r"^[a-z][a-z0-9+.-]*:|^#|^/", href):
        return href
    path, sep, frag = href.partition("#")
    rel = os.path.relpath(os.path.normpath(os.path.join(page_dir, path)), root)
    if rel.startswith(".."):  # leaves the book: a repo file outside docs/
        repo_path = os.path.normpath(os.path.join("docs", rel)).replace(os.sep, "/")
    elif rel.startswith("superpowers" + os.sep):  # not published
        repo_path = "docs/" + rel.replace(os.sep, "/")
    else:
        return href
    if repo_path.endswith(".html"):  # mdBook renamed a Markdown link
        repo_path = repo_path[: -len(".html")] + ".md"
    return REPO + repo_path + sep + frag


def main(root: str) -> int:
    root = os.path.abspath(root)
    broken = []
    for dirpath, _, names in os.walk(root):
        for name in names:
            if not name.endswith(".html"):
                continue
            page = os.path.join(dirpath, name)
            with open(page, encoding="utf-8") as f:
                html = f.read()
            html = HREF.sub(lambda m: 'href="%s"' % rewrite(dirpath, root, m.group(1)), html)
            with open(page, "w", encoding="utf-8") as f:
                f.write(html)
            for href in HREF.findall(html):
                if re.match(r"^[a-z][a-z0-9+.-]*:|^#|^/", href):
                    continue
                target = os.path.join(dirpath, href.partition("#")[0].partition("?")[0])
                if href.partition("#")[0] and not os.path.exists(target):
                    broken.append("%s: broken link %s" % (os.path.relpath(page, root), href))
    print("\n".join(broken))
    return 1 if broken else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1]))
