#!/usr/bin/env python3
"""Check pinned guidance and local links without third-party dependencies."""

import hashlib
import json
from pathlib import Path
import re
import sys
from urllib.parse import unquote, urlsplit


def skill_metadata(text):
    """Parse only the two plain single-line YAML scalars used by pinned skills.

    Reject other YAML syntax rather than pretending to be a general YAML parser.
    """
    lines = text.splitlines()
    if not lines or lines[0] != "---" or "---" not in lines[1:]:
        raise ValueError("missing YAML front matter")
    metadata = {}
    for line in lines[1 : lines[1:].index("---") + 1]:
        match = re.fullmatch(r"(name|description): ([^\s].*)", line)
        if not match or match[1] in metadata:
            raise ValueError("expected unique name/description plain scalar")
        value = match[2]
        if value[0] in "[]{}|>!&*'\"#%@`" or ": " in value or " #" in value:
            raise ValueError("unsupported YAML scalar syntax")
        metadata[match[1]] = value
    if set(metadata) != {"name", "description"}:
        raise ValueError("name and description required")
    if not re.fullmatch(r"[a-z0-9]+(?:-[a-z0-9]+)*", metadata["name"]):
        raise ValueError("invalid skill name")
    return metadata


def check(root):
    root = root.resolve()
    errors = []
    manifest = root / "docs/engineering/skill-sources.json"
    try:
        entries = json.loads(manifest.read_text())["files"]
        if not isinstance(entries, list) or not entries:
            raise ValueError("files must be a nonempty list")
        pinned = set((root / ".agents/skills").rglob("*.md"))
        pinned.update((root / "docs/engineering/upstream-prompts").rglob("*.md"))
        license_path = root / "docs/engineering/upstream-license.txt"
        if license_path.exists():
            pinned.add(license_path)
        listed = set()
        for entry in entries:
            path = (root / entry["local"]).resolve()
            if path in listed:
                errors.append(f"duplicate manifest file: {entry['local']}")
            listed.add(path)
            if not path.is_relative_to(root) or not path.is_file():
                errors.append(f"missing/outside manifest file: {entry['local']}")
            elif hashlib.sha256(path.read_bytes()).hexdigest() != entry["sha256"]:
                errors.append(f"SHA-256 mismatch: {entry['local']}")
        if pinned != listed:
            errors.append(
                "manifest must cover exactly the vendored skills, prompts and license"
            )
    except (OSError, ValueError, KeyError, TypeError) as error:
        errors.append(f"invalid manifest: {error}")

    for path in sorted((root / ".agents/skills").glob("*/SKILL.md")):
        try:
            metadata = skill_metadata(path.read_text())
            if metadata["name"] != path.parent.name:
                errors.append(f"skill name differs from directory: {path}")
        except ValueError as error:
            errors.append(f"{path}: {error}")

    documents = [root / name for name in ("AGENTS.md", "README.md", "CONTRIBUTING.md")]
    for path in documents:
        if not path.is_file():
            errors.append(f"missing required document: {path.name}")
    documents += [
        path
        for path in (root / "docs").rglob("*.md")
        if not path.is_relative_to(root / "docs/engineering/upstream-prompts")
    ]
    documents += list((root / ".github").rglob("*.md"))
    documents += list((root / ".agents/skills").rglob("*.md"))
    for path in documents:
        if not path.exists():
            continue
        # Code examples are not Markdown links; leave upstream prompt templates alone.
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
    print("guidance: metadata, SHA-256 and local links OK")
