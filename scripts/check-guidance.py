#!/usr/bin/env python3
"""Check project guidance links and plugin-only delivery without dependencies."""

from pathlib import Path
import re
import sys
from urllib.parse import unquote, urlsplit


def check(root):
    root = root.resolve()
    errors = []
    copies = list((root / ".agents/skills").rglob("SKILL.md"))
    copies += list((root / ".codex/agents").rglob("*.toml"))
    for path in sorted(copies):
        errors.append(f"copied guidance is not supported: {path.relative_to(root)}")

    documents = [root / name for name in ("AGENTS.md", "README.md", "CONTRIBUTING.md")]
    for path in documents:
        if not path.is_file():
            errors.append(f"missing required document: {path.name}")
    documents += list((root / "docs").rglob("*.md"))
    documents += list((root / ".github").rglob("*.md"))
    documents += list((root / ".agents").rglob("*.md"))
    for path in documents:
        if not path.exists():
            continue
        # Code examples are not Markdown links.
        text = re.sub(r"```.*?```", "", path.read_text(), flags=re.S)
        for target in re.findall(r"\[[^\]\n]+\]\(([^\s)]+)(?:\s+\"[^\"]*\")?\)", text):
            target = target.strip("<>")
            url = urlsplit(target)
            if url.scheme or url.netloc or not url.path:
                continue
            destination = path.parent / unquote(url.path)
            if not destination.exists():
                errors.append(f"{path.relative_to(root)}: missing local link: {target}")
    return errors


if __name__ == "__main__":
    failures = check(Path(__file__).resolve().parents[1])
    if failures:
        print("\n".join(failures), file=sys.stderr)
        sys.exit(1)
    print("guidance: required documents, local links and plugin-only delivery OK")
