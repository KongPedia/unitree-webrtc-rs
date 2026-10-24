#!/usr/bin/env python3
"""Validate local release identity before building/uploading; never mutate or publish."""

import argparse
import hashlib
import re
import sys
import zipfile
from email.parser import BytesParser
from pathlib import Path

# Python 3.10 uses tomli from the existing locked development environment.
try:
    import tomllib
except ImportError:
    import tomli as tomllib


def normalized_name(name):
    return re.sub(r"[-_.]+", "-", name).lower()


def load_toml(root, filename):
    try:
        return tomllib.loads((root / filename).read_text(encoding="utf-8"))
    except (OSError, ValueError) as error:
        raise ValueError(f"{filename}: cannot read valid TOML") from error


def project_identity(root):
    identities = {}
    for filename, section in (("Cargo.toml", "package"), ("pyproject.toml", "project")):
        project = load_toml(root, filename).get(section, {})
        name, version = project.get("name"), project.get("version")
        if not isinstance(name, str) or not isinstance(version, str) or not version.strip():
            raise ValueError(f"{filename}: missing project name/version")
        identities[filename] = (normalized_name(name), version)
    name, version = identities["Cargo.toml"]
    if identities["pyproject.toml"] != (name, version):
        raise ValueError("Cargo.toml and pyproject.toml project name/version mismatch")
    for filename in ("Cargo.lock", "uv.lock"):
        entries = [
            entry
            for entry in load_toml(root, filename).get("package", [])
            if normalized_name(entry.get("name", "")) == name
            and ("source" not in entry if filename == "Cargo.lock" else entry.get("source") == {"editable": "."})
        ]
        if len(entries) != 1:
            raise ValueError(f"{filename}: expected one local project version entry")
        if entries[0].get("version") != version:
            raise ValueError(f"{filename}: project version mismatch")
    return name, version


def wheel_identity(path, name, version):
    try:
        with zipfile.ZipFile(path) as archive:
            metadata_files = [entry for entry in archive.namelist() if entry.endswith(".dist-info/METADATA")]
            if len(metadata_files) != 1:
                raise ValueError("expected one wheel METADATA file")
            metadata = BytesParser().parsebytes(archive.read(metadata_files[0]))
        names, versions = metadata.get_all("Name", []), metadata.get_all("Version", [])
        if len(names) != 1 or normalized_name(names[0]) != name:
            raise ValueError("wheel METADATA project name mismatch")
        if versions != [version]:
            raise ValueError("wheel METADATA version mismatch")
        with path.open("rb") as artifact:
            digest = hashlib.sha256()
            for block in iter(lambda: artifact.read(1024 * 1024), b""):
                digest.update(block)
        return f"SHA-256 {path.name}: {digest.hexdigest()}"
    except (OSError, ValueError, KeyError, zipfile.BadZipFile) as error:
        raise ValueError(f"wheel {path.name}: {error}") from error


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=Path(__file__).resolve().parents[1])
    parser.add_argument(
        "--tag",
        help="require exactly v<project-version>; omit for manual/local validation",
    )
    parser.add_argument(
        "--wheel",
        action="append",
        type=Path,
        default=[],
        help="validate each built wheel METADATA",
    )
    args = parser.parse_args(argv)
    try:
        name, version = project_identity(args.root)
        if args.tag is not None and args.tag != f"v{version}":
            raise ValueError("release tag must exactly match v<project-version>")
        reports = [wheel_identity(path, name, version) for path in args.wheel]
    except ValueError as error:
        print(f"release check failed: {error}", file=sys.stderr)
        return 1
    print(f"release metadata OK: {name} {version} (Cargo/pyproject/Cargo.lock/uv.lock)")
    for report in reports:
        print(report)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
