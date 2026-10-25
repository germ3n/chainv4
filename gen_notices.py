import json
import os
import re
import subprocess
import sys

ROOT = os.path.dirname(os.path.abspath(__file__))
NOTICES = os.path.join(ROOT, "THIRD_PARTY_NOTICES.txt")
RULE = "-" * 80

# What ships, and the cargo workspace each is built from. The ui crate is in the apps' own.
BUILDS = [
    (".", "node"),
    ("explorer", "explorer"),
    ("wallet", "wallet"),
]


def section(title):
    return ["", RULE, title, RULE, ""]


def read(path):
    with open(path, encoding="utf-8", errors="replace") as f:
        return f.read().replace("\r\n", "\n").replace("\r", "\n")


def norm_text(text):
    lines = [line.rstrip() for line in text.split("\n")]
    out = []
    for line in lines:
        if line == "" and (not out or out[-1] == ""):
            continue
        out.append(line)
    while out and out[-1] == "":
        out.pop()

    return "\n".join(out)


def metadata(workspace):
    return json.loads(subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--locked", "--all-features"],
        cwd=os.path.join(ROOT, workspace), check=True, stdout=subprocess.PIPE,
    ).stdout)


def ours(p):
    # Path packages inside this repository are chainv4 itself; a path package elsewhere, such as a
    # patched-in fork, is third-party like any other.
    return os.path.abspath(p["manifest_path"]).startswith(ROOT + os.sep)


crates = {}
used_by = {}
local = set()
for workspace, build in BUILDS:
    meta = metadata(workspace)
    by_key = {(p["name"], p["version"]): p for p in meta["packages"]}
    lock = read(os.path.join(ROOT, workspace, "Cargo.lock"))
    for name, version in re.findall(r'\[\[package\]\]\nname = "([^"]+)"\nversion = "([^"]+)"', lock):
        if (name, version) not in by_key:
            sys.exit("missing metadata for %s %s in %s" % (name, version, workspace))
        p = by_key[(name, version)]
        if ours(p):
            local.add(name)
            continue
        crates[(name, version)] = p
        used_by.setdefault((name, version), [])
        if build not in used_by[(name, version)]:
            used_by[(name, version)].append(build)

crates = sorted(crates.values(), key=lambda p: (p["name"].lower(), [(0, int(x), "") if x.isdigit() else (1, 0, x) for x in re.split(r"[.+-]", p["version"])]))


def crate(name):
    for p in crates:
        if p["name"] == name:
            return p
    sys.exit("no Cargo.lock contains %s any longer; update gen_notices.py" % name)


def crate_dir(p):
    return os.path.dirname(p["manifest_path"])


def bundled(p, *parts):
    path = os.path.join(crate_dir(p), *parts)
    if not os.path.isfile(path):
        sys.exit("%s no longer bundles %s; update gen_notices.py" % (label(p), os.path.join(*parts)))

    return norm_text(read(path))


def label(p):
    return "%s %s" % (p["name"], p["version"])


def expr(p):
    lic = p["license"] or ""
    lic = re.sub(r"\s*/\s*", " OR ", lic)

    return lic


def choices(p):
    lic = expr(p)
    if " AND " in lic:
        return None

    return [c.strip("() ") for c in lic.split(" OR ")]


LICENSE_NAME = re.compile(r"(?i)^(licen[cs]e|copying|copyright|notice|unlicense)")


def license_files(p):
    d = crate_dir(p)
    files = [f for f in os.listdir(d) if LICENSE_NAME.match(f) and os.path.isfile(os.path.join(d, f))]
    rank = lambda f: (0 if "MIT" in f.upper() else 1 if f.upper() in ("LICENSE", "LICENSE.MD", "LICENSE.TXT") else 2, f)

    return [os.path.join(d, f) for f in sorted(files, key=rank)]


TEMPLATE = re.compile(
    r"(?i)\[yyyy\]|\{yyyy\}|<year>|\[year\]|\{year\}|<copyright|\[name of|\{name of|"
    r"copyright (notice|license|owner|holder|statement|and permission|and license|and related)|"
    r"copyright notices|copyright,? patent|copyright law|copyright interest|subject to copyright|"
    r"copyright assignment|copyright to\b|copyright in\b"
)
COPYRIGHT_LINE = re.compile(r"(?i)^[\s#*/;!-]*((copyright\b|©).*)$")


def copyright_lines(paths):
    out = []
    for path in paths:
        try:
            text = read(path)
        except OSError:
            continue
        for line in text.split("\n"):
            m = COPYRIGHT_LINE.match(line)
            if not m:
                continue
            s = re.sub(r"\s+", " ", m.group(1)).strip().rstrip(",;")
            s = re.sub(r"\s+or$", "", s)
            if TEMPLATE.search(s) or len(s) < 12:
                continue
            if not re.search(r"\d{4}|[A-Z][a-z]", s[9:]):
                continue
            if s not in out:
                out.append(s)

    return out


def authors(p):
    names = [re.sub(r"\s*<[^>]*>", "", a).strip().rstrip(".") for a in p.get("authors") or []]

    return [n for n in names if n]


NOTES = {
    "android-activity": "Rust code is used under the MIT terms. The bundled GameActivity C/C++\n  sources are Apache-2.0; see the Android GameActivity section.",
    "dpi": "Both licenses apply. The Apache-2.0 text is above; the MIT notice for\n  code taken from rust-lang/libm is in the copyright lines here.",
    "epaint_default_fonts": "Crate code is used under the MIT terms. The bundled fonts are listed\n  in the egui default fonts section.",
    "luau0-src": "Bundles the Luau VM and compiler, from germ3n's fork, which adds fuel\n  metering and deterministic tables; MIT, with the Roblox and Lua.org notices above.",
    "mlua-sys": "From germ3n's fork of mlua; MIT.",
    "librocksdb-sys": "Crate code is used under the MIT terms. It bundles RocksDB, used under\n  Apache-2.0, and Snappy; see the RocksDB section.",
    "lz4-sys": "Bundles the LZ4 library; see the compression libraries section.",
    "libz-sys": "Bundles zlib; see the compression libraries section.",
    "bzip2-sys": "Bundles libbzip2; see the compression libraries section.",
    "zstd-sys": "Bundles the Zstandard library; see the compression libraries section.",
    "r-efi": "Used under the MIT terms. The LGPL-2.1-or-later option is not used.",
    "unicode-ident": "Crate code is used under the MIT terms. The Unicode data is under the\n  Unicode License V3 in the section above.",
}


def needs_full_text(p):
    c = choices(p)
    if c is None:
        parts = re.split(r"\s+(?:AND|OR)\s+", expr(p).replace("(", "").replace(")", ""))

        return any(x not in ("MIT", "Apache-2.0") for x in parts)
    if "MIT" in c or all(x.startswith("Apache-2.0") for x in c):
        return False
    if "Apache-2.0" in c:
        return False

    return c[0] not in ("CC0-1.0", "WTFPL", "Unlicense")


def full_text_files(p):
    out = []
    for path in license_files(p):
        text = read(path)
        name = os.path.basename(path).upper()
        if "APACHE" in name or re.match(r"\s*Apache License", text):
            continue
        if "MIT" in name and p["name"] in ("unicode-ident", "dpi"):
            continue
        out.append(path)

    return out


def extra_text(p):
    # clipboard-win names error-code's license as its own.
    if p["name"] == "clipboard-win":
        return [os.path.join(crate_dir(crate("error-code")), "LICENSE")]

    return []


groups = []
for p in crates:
    if p["name"] == "epaint_default_fonts" or not needs_full_text(p):
        continue
    if p["name"] == "zstd-sys":
        files = [f for f in full_text_files(p) if not f.endswith("LICENSE.BSD-3-Clause")]
    else:
        files = full_text_files(p) or extra_text(p)
    if not files:
        sys.exit("no license text for %s" % label(p))
    text = "\n\n".join(norm_text(read(f)) for f in files)
    for g in groups:
        if g["text"] == text:
            g["crates"].append(p)
            break
    else:
        groups.append({"text": text, "crates": [p], "expr": expr(p)})


def apache_text():
    counts = {}
    for p in crates:
        for path in license_files(p):
            if "APACHE" not in os.path.basename(path).upper():
                continue
            text = norm_text(read(path))
            stop = text.find("END OF TERMS AND CONDITIONS")
            if text.lstrip().startswith("Apache License") and stop >= 0:
                text = text[: stop + len("END OF TERMS AND CONDITIONS")]
                counts[text] = counts.get(text, 0) + 1
    if not counts:
        sys.exit("no crate in the Cargo.lock files ships the Apache-2.0 text")

    return max(counts, key=lambda text: (counts[text], text))


def join_names(names):
    if len(names) == 1:
        return names[0]

    return ", ".join(names[:-1]) + " and " + names[-1]


def wrap(text, width=80, indent=""):
    words = text.split()
    lines = []
    cur = indent
    for w in words:
        if len(cur) + len(w) + (0 if cur.strip() == "" else 1) > width:
            lines.append(cur.rstrip())
            cur = indent + w
        else:
            cur = cur + ("" if cur.strip() == "" else " ") + w
    if cur.strip():
        lines.append(cur.rstrip())

    return lines


def ofl_parts(path):
    text = read(path)
    marker = "SIL OPEN FONT LICENSE"
    stop = text.find(marker)
    if stop < 0:
        sys.exit("%s has no OFL text" % path)
    head = []
    for line in text[:stop].split("\n"):
        line = line.strip()
        if not line or set(line) <= set("-"):
            continue
        head.append(line)
    body = "\n".join(line.rstrip() for line in text[stop:].split("\n")).strip()

    return head, body


luau = crate("luau0-src")
rocksdb = crate("librocksdb-sys")
lz4 = crate("lz4-sys")
zstd_sys = crate("zstd-sys")
libz = crate("libz-sys")
bzip2 = crate("bzip2-sys")
fonts = crate("epaint_default_fonts")
activities = [p for p in crates if p["name"] == "android-activity"]

out = [
    "Third-party notices",
    "",
    "Distribute this file with the chainv4 node and with the explorer and the wallet,",
    "on any platform they are built for: desktop, Android and the web. It collects",
    "the license notices for the code and the fonts compiled into them. chainv4's",
    "own license is also reproduced so this file can stand alone next to a binary.",
    "",
    "Crate versions below are the ones pinned in the Cargo.lock files of the node",
    "(the repository root), the explorer and the wallet. License texts were read from",
    "those crate versions.",
]

out += section("chainv4")
out += [
    "Source: this repository",
    "License: MIT",
    "",
    norm_text(read(os.path.join(ROOT, "LICENSE"))),
]

out += section("The Apache License, Version 2.0")
out += wrap("Reproduced once here. It covers RocksDB, Android GameActivity and every Apache-2.0 crate listed further down.")
out += ["", apache_text()]

out += section("Luau")
out += [
    "Source: the Luau VM and compiler, vendored by %s from" % label(luau),
    "         https://github.com/germ3n/luau-src-rs, compiled into the node",
    "License file reproduced from the crate:",
    "",
    norm_text(read(os.path.join(crate_dir(luau), "LICENSE"))),
]

out += section("RocksDB")
out += wrap(
    "Bundled in %s and compiled into the node as its database. RocksDB is offered under the GPLv2 or "
    "the Apache License, Version 2.0; this distribution uses Apache-2.0, reproduced above. Parts of it "
    "come from LevelDB, whose notice follows, and it is built with Snappy, whose license follows that."
    % label(rocksdb)
)
out += ["", "LevelDB:", "", bundled(rocksdb, "rocksdb", "LICENSE.leveldb")]
out += ["", "Snappy:", "", bundled(rocksdb, "snappy", "COPYING")]

out += section("Compression libraries")
out += wrap("RocksDB compresses with these, each bundled in its -sys crate and compiled into the node.")
for title, p, parts in [
    ("LZ4, in %s" % label(lz4), lz4, ("liblz4", "lib", "LICENSE")),
    ("Zstandard, in %s. The zstd command-line program is not compiled in." % label(zstd_sys), zstd_sys, ("zstd", "LICENSE")),
    ("zlib, in %s" % label(libz), libz, ("src", "zlib", "LICENSE")),
    ("bzip2, in %s" % label(bzip2), bzip2, ("bzip2-1.0.8", "LICENSE")),
]:
    out += ["", "=" * 79, title, "=" * 79, "", bundled(p, *parts)]

out += section("Android GameActivity")
ga_lines = []
for p in activities:
    for r, ds, fs in os.walk(crate_dir(p)):
        for f in fs:
            if f.endswith((".c", ".cpp", ".h")):
                for line in copyright_lines([os.path.join(r, f)]):
                    if line not in ga_lines:
                        ga_lines.append(line)
out += wrap(
    "Bundled in %s as C/C++ sources from the Android Games SDK. Only compiled into the explorer and "
    "the wallet for Android." % join_names([label(p) for p in activities])
)
out += [
    "License: Apache License, Version 2.0 (text above)",
    "",
] + sorted(ga_lines)

# Fonts compiled into the apps, and embedded in the wiki and docs pages the node's repository
# builds. Each is under the SIL Open Font License; the license text is reproduced once.
app_fonts = [
    ("IBM Plex Sans and IBM Plex Mono", "Regular and SemiBold, and Mono Regular. Compiled into the explorer.", "explorer/assets/fonts/ibm-plex-OFL.txt"),
    ("Manrope", "Regular and SemiBold. Compiled into the wallet, and embedded in the wiki and docs pages.", "wallet/assets/fonts/manrope-OFL.txt"),
    ("Newsreader", "Regular, and Italic in the wiki. Compiled into the wallet, and embedded in the wiki and docs pages.", "wallet/assets/fonts/newsreader-OFL.txt"),
    ("JetBrains Mono", "Regular. Compiled into the wallet, and embedded in the wiki and docs pages.", "wallet/assets/fonts/jetbrains-OFL.txt"),
]
ofl_body = None
font_entries = []
for title, note, path in app_fonts:
    head, body = ofl_parts(os.path.join(ROOT, path))
    if ofl_body is None:
        ofl_body = body
    elif body != ofl_body:
        sys.exit("the OFL texts of the bundled fonts diverged; update gen_notices.py")
    font_entries.append((title, note, head))
for path in ("wiki/fonts/manrope-OFL.txt", "wiki/fonts/newsreader-OFL.txt", "wiki/fonts/jetbrains-OFL.txt"):
    if ofl_parts(os.path.join(ROOT, path))[1] != ofl_body:
        sys.exit("%s no longer matches the apps' copy; update gen_notices.py" % path)

out += section("Fonts")
out += [
    "Sources: explorer/assets/fonts, wallet/assets/fonts and wiki/fonts",
    "Reserved font names are unchanged. The wiki's files are Latin subsets.",
    "License: SIL Open Font License, Version 1.1",
    "",
]
for title, note, head in font_entries:
    out += [title] + wrap(note) + head + [""]
out.append(ofl_body)

out += section("egui default fonts")
out += [
    "Bundled in %s and compiled into the explorer and the wallet." % label(fonts),
    "",
]
font_files = [
    ("Hack Regular", "Hack-Regular.txt"),
    ("Ubuntu Light, Ubuntu Font Licence 1.0", "UFL.txt"),
    ("Noto Emoji, SIL Open Font License 1.1", "OFL.txt"),
    ("emoji-icon-font, MIT", "emoji-icon-font-mit-license.txt"),
    ("egui icons, a subset of emoji-icon-font", "egui-icons.txt"),
]
for idx, (title, f) in enumerate(font_files):
    if idx:
        out.append("")
    out += ["=" * 79, title, "=" * 79, "", bundled(fonts, "fonts", f)]

out += section("Other license texts the Cargo.lock files require")
out += [
    "These texts are in addition to the MIT and Apache-2.0 texts above. A crate is",
    "named under the text that applies to it.",
]
for g in groups:
    out += [""] + wrap(join_names([label(p) for p in g["crates"]])) + ["  " + g["expr"], "", g["text"]]

out += section("Rust crates from the Cargo.lock files")
local_names = sorted(local)
out += wrap(
    "Every third-party crate in the Cargo.lock files of the node, the explorer and the wallet is listed "
    "below, for every target platform, with what it is built into. Local crates %s are chainv4 itself, "
    "covered by the MIT license at the top." % join_names(local_names)
)
out += [""]
out += wrap(
    "Where a crate offers a choice, this distribution uses MIT if MIT is one of the choices, "
    "and otherwise the license noted for that crate. MIT-licensed crates use the MIT terms "
    "reproduced at the top with the copyright lines given here in place of chainv4's. "
    "Apache-2.0 crates use the Apache License text above."
)
out += ["", "%d crates." % len(crates)]

for p in crates:
    files = license_files(p)
    lines = copyright_lines(files)
    if p["name"] == "zstd-sys":
        lines = [l for l in lines if "Meta Platforms" not in l and "Facebook" not in l]
    out += ["", label(p), "  " + expr(p), "  Built into: " + join_names(used_by[(p["name"], p["version"])])]
    if lines:
        out += ["  " + l for l in lines]
    else:
        names = authors(p)
        if names:
            out += wrap("Authors: " + ", ".join(names) + ". The package has no separate copyright line.", indent="  ")
        else:
            out.append("  %s contributors. The package has no separate copyright line." % p["name"])
    if p["name"] in NOTES:
        out.append("  " + NOTES[p["name"]])

text = "\n".join(out).rstrip("\n") + "\n"
open(NOTICES, "w", encoding="utf-8").write(text)
print(len(crates), "crates,", len(groups), "text groups,", text.count("\n"), "lines")
