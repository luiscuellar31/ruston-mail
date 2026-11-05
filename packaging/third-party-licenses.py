#!/usr/bin/env python3
"""Write THIRD_PARTY_LICENSES.txt: the notices of the software Ruston Mail
ships beside its own, which the apps and their packages carry.

    python3 packaging/third-party-licenses.py          # write the file
    python3 packaging/third-party-licenses.py --check  # fail if it is stale

It lists the Rust crates the workspace's binaries depend on, on each
platform the release packages, each under the least demanding license it
offers: one that asks nothing of a binary leaves the crate out, MIT or ISC
gives that text alone, and anything else every license file the crate
publishes. It adds the notices of the icons and of egui's fonts, and
ends by acknowledging protonmail-rs, which Ruston Mail was originally
based on. Crates that only run while compiling, procedural macros, are
left out. Run it again after changing dependencies; CI checks that it
matches.
"""

import json
import pathlib
import re
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
OUT = ROOT / "THIRD_PARTY_LICENSES.txt"
# The names a crate gives its license files, at its root.
LICENSE_PREFIXES = ("LICENSE", "LICENCE", "COPYING", "COPYRIGHT", "NOTICE", "UNLICENSE")
# Licenses that ask nothing of a binary, Zlib's and BSL-1.0's notice being
# for sources only: a crate that offers one needs no notice here.
NO_NOTICE = {"0BSD", "BSL-1.0", "CC0-1.0", "MIT-0", "Unlicense", "Zlib"}
# The sentences that tell an MIT or an ISC text, whatever its file is named.
SHORT_LICENSE = re.compile(
    r"permission is hereby granted, free of charge, to any person obtaining a copy"
    r"|permission to use, copy, modify, and(/or)? distribute this software for any purpose"
)
# What the release workflow builds, so one file serves every package.
TARGETS = (
    "aarch64-apple-darwin",
    "x86_64-apple-darwin",
    "aarch64-unknown-linux-gnu",
    "x86_64-unknown-linux-gnu",
    "aarch64-pc-windows-msvc",
    "x86_64-pc-windows-msvc",
)


def metadata(target):
    listed = subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--locked", "--filter-platform", target],
        cwd=ROOT,
        check=True,
        capture_output=True,
        text=True,
    )
    return json.loads(listed.stdout)


def is_proc_macro(package):
    return any("proc-macro" in target["kind"] for target in package["targets"])


def shipped_packages():
    """The packages the workspace members depend on as normal dependencies,
    on any of TARGETS, without procedural macros and what only they need;
    the members, Ruston Mail's own, are left out."""
    shipped = {}
    for target in TARGETS:
        meta = metadata(target)
        nodes = {node["id"]: node for node in meta["resolve"]["nodes"]}
        packages = {package["id"]: package for package in meta["packages"]}
        members = set(meta["workspace_members"])
        seen = set()
        pending = list(members)
        while pending:
            for dep in nodes[pending.pop()]["deps"]:
                normal = any(kind["kind"] is None for kind in dep["dep_kinds"])
                package = packages[dep["pkg"]]
                if normal and dep["pkg"] not in seen and not is_proc_macro(package):
                    seen.add(dep["pkg"])
                    pending.append(dep["pkg"])
        for id in seen - members:
            shipped[id] = packages[id]
    return sorted(shipped.values(), key=lambda package: (package["name"], package["version"]))


def text_of(path):
    """A license file's text, without the spaces that end some of its
    lines, so the file passes `git diff --check`."""
    text = path.read_text(encoding="utf-8", errors="replace").replace("\r\n", "\n")
    lines = (line.rstrip() for line in text.strip().split("\n"))
    return "\n".join(lines) + "\n"


def license_files(package):
    folder = pathlib.Path(package["manifest_path"]).parent
    return sorted(
        path
        for path in folder.iterdir()
        if path.is_file() and path.name.upper().startswith(LICENSE_PREFIXES)
    )


def offered(package):
    """The licenses a crate offers, any one of which may be chosen, or None
    when it asks for several at once (AND) or names none."""
    expression = (package.get("license") or "").replace("/", " OR ")
    if not expression or " AND " in expression:
        return None
    return {option.strip(" ()").split(" WITH ")[0].strip() for option in expression.split(" OR ")}


def notice_files(package):
    """The license files a crate's notice needs, under the least demanding
    license it offers: None when it offers one that asks nothing of a
    binary; its MIT or ISC text alone when it offers either and publishes
    it; every license file it publishes otherwise."""
    licenses = offered(package)
    files = license_files(package)
    if licenses and licenses & NO_NOTICE:
        return None
    if licenses and licenses & {"MIT", "ISC"}:
        short = [path for path in files if SHORT_LICENSE.search(" ".join(text_of(path).lower().split()))]
        if short:
            return short
    return files


def section(title):
    return f"{title}\n{'=' * len(title)}\n"


def notices():
    packages = shipped_packages()
    fonts = next(p for p in packages if p["name"] == "epaint_default_fonts")
    font_folder = pathlib.Path(fonts["manifest_path"]).parent / "fonts"
    out = [
        section("Third-party notices for Ruston Mail"),
        "Ruston Mail is under the MIT License in LICENSE. It includes the software\n"
        "below, under the licenses that follow. This file is generated by\n"
        "packaging/third-party-licenses.py.\n",
        section("Icons"),
        "Bootstrap Icons, https://icons.getbootstrap.com, in assets/icons/bootstrap:\n",
        text_of(ROOT / "assets" / "icons" / "bootstrap" / "LICENSE"),
        section("Fonts"),
        f"egui's default fonts, from {fonts['name']} {fonts['version']}: Hack, Ubuntu\n"
        "Light, Noto Emoji and the emoji icon font.\n",
    ]
    for path in sorted(font_folder.glob("*.txt")):
        out.append(f"--- {path.name} ---\n\n{text_of(path)}")

    # Each text once, after the list, with the crates that publish it.
    texts = {}
    rows = []
    listed = 0
    for package in packages:
        files = notice_files(package)
        if files is None:
            continue
        listed += 1
        for path in files:
            texts.setdefault(text_of(path), []).append(f"{package['name']} {package['version']}")
        license = package.get("license") or package.get("license_file") or "unknown"
        rows.append(f"{package['name']} {package['version']}: {license}")
        if not files:
            # Its notice is its authors' under the license it names.
            authors = ", ".join(package.get("authors") or []) or "its authors"
            source = package.get("repository") or package.get("homepage") or ""
            rows.append(f"    publishes no license file; by {authors} {source}".rstrip())
    out.append(section(f"Rust crates ({listed})"))
    out.append("\n".join(rows) + "\n")
    out.append(section("License texts of the Rust crates"))
    for text, users in texts.items():
        out.append(f"--- Published by: {', '.join(users)} ---\n\n{text}")
    out.append(section("Acknowledgements"))
    out.append(
        "Ruston Mail was originally based on protonmail-rs, by Filippo Finke:\n"
        "https://github.com/filippofinke/protonmail-rs\n"
    )
    return "\n".join(out)


def main():
    generated = notices()
    if sys.argv[1:] == ["--check"]:
        current = OUT.read_text(encoding="utf-8") if OUT.exists() else ""
        if current != generated:
            sys.exit(f"{OUT.name} is stale: run python3 packaging/third-party-licenses.py")
        return
    OUT.write_text(generated, encoding="utf-8", newline="\n")
    print(f"wrote {OUT.name}: {len(generated.encode())} bytes")


if __name__ == "__main__":
    main()
