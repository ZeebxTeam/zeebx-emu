#!/usr/bin/env python3
"""Rewrites the README's "Who makes Zeebx" list with the GitHub contributors.

It is the same list as the site's (`docs/site.js`, `buscarContribuidores`): the repository's
"Contributors" tab, without the bots. The site fetches it on every visit; the README runs nothing,
so the list stays written in it, between the `<!-- contributors -->` and `<!-- /contributors -->`
markers, and is redone by this script when new people come in.

    python3 tools/contributors.py
"""

import json
import pathlib
import urllib.request

REPOSITORY = "ZeebxTeam/zeebx-emu"
README = pathlib.Path(__file__).resolve().parent.parent / "README.md"
START, END = "<!-- contributors -->", "<!-- /contributors -->"
PER_ROW = 6


def contributors() -> list[dict]:
    request = urllib.request.Request(
        f"https://api.github.com/repos/{REPOSITORY}/contributors?per_page=100",
        headers={"Accept": "application/vnd.github+json", "User-Agent": "zeebx-readme"},
    )
    with urllib.request.urlopen(request, timeout=30) as response:
        return [c for c in json.load(response) if c["type"] == "User"]


# The format is all-contributors', and not out of taste: GitHub's table sizes each column by what
# is in it, and without the fixed width a long name leaves the columns uneven. In the editor's
# preview it looks fine either way; on the repository page, it does not.
def table(people: list[dict]) -> str:
    width = f"{100 / PER_ROW:.2f}%"
    cells = [
        f'<td align="center" valign="top" width="{width}"><a href="{p["html_url"]}">'
        f'<img src="{p["avatar_url"]}&s=100" width="100px;" alt="{p["login"]}"/><br />'
        f'<sub><b>{p["login"]}</b></sub></a></td>'
        for p in people
    ]
    rows = [cells[i : i + PER_ROW] for i in range(0, len(cells), PER_ROW)]
    body = "\n".join("  <tr>\n    " + "\n    ".join(row) + "\n  </tr>" for row in rows)
    return f"<table>\n{body}\n</table>"


def main() -> None:
    text = README.read_text(encoding="utf-8")
    before, rest = text.split(START, 1)
    _, after = rest.split(END, 1)
    people = contributors()
    README.write_text(f"{before}{START}\n{table(people)}\n{END}{after}", encoding="utf-8")
    print(f"README: {len(people)} contributors")


if __name__ == "__main__":
    main()
