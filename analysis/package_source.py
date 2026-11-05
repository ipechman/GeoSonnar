"""Export a reviewed, text-only GeoSonnar source tree without local recordings.

Usage: python analysis/package_source.py --output dist/source --zip dist/source.zip
Both destinations must be new. No Git repository or third-party Python package
is required. The manifest identifies included bytes, not excluded private paths.
This conservative screen supplements a human review; it cannot identify every
possible secret or private fact in otherwise ordinary source text.
"""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import stat
import sys
import tempfile
import zipfile


ROOT = Path(__file__).resolve().parents[1]
TOP_FILES = (
    ".gitignore", ".gitattributes", "Cargo.toml", "Cargo.lock", "LICENSE",
    "README.md", "THIRD_PARTY_NOTICES.md",
)
OPTIONAL_TOP_FILES = ("requirements-research.txt", "CONTRIBUTING.md")
SOURCE_DIRS = ("src", "tests", "analysis", "benchmarks", "cases", "docs", "examples", "tools")
MANIFEST = "SOURCE_MANIFEST.json"
MAX_FILE_BYTES = 10 * 1024 * 1024
EXCLUDED_RECEIPT = "benchmarks/native_search_sensitivity_contract.json"
EXCLUDED_DIRS = {
    "__pycache__", ".git", ".hg", ".svn", ".aws", ".ssh", ".codex", ".agents",
    ".venv", "venv", "node_modules", ".pytest_cache", ".mypy_cache", ".ruff_cache",
    "target", "output", "dist", "tmp", "temp", "cache", "caches", "data",
    "private", "private-validation", "private_validation", "recordings", "media",
    "original video", "known videos with coordinates",
}
EXCLUDED_EXTENSIONS = {
    ".pyc", ".pyo", ".exe", ".dll", ".pdb", ".so", ".dylib", ".a", ".lib",
    ".o", ".obj", ".bin", ".dat", ".db", ".sqlite", ".sqlite3", ".log",
    ".wav", ".mp3", ".mp4", ".mkv", ".mov", ".avi", ".webm", ".m4a",
    ".flac", ".ogg", ".aac", ".opus", ".png", ".jpg", ".jpeg", ".webp",
    ".gif", ".bmp", ".ico", ".svg", ".tif", ".tiff", ".pdf", ".docx",
    ".xlsx", ".pptx", ".zip", ".gz", ".bz2", ".xz", ".tar", ".7z", ".rar",
    ".npz", ".npy", ".f32", ".gspec", ".parquet", ".hgt", ".grib", ".nc",
    ".pem", ".key", ".p12", ".pfx", ".keystore", ".bak", ".tmp", ".swp",
}
TEXT_EXTENSIONS = {
    ".rs", ".py", ".json", ".md", ".txt", ".csv", ".html", ".cjs", ".js",
    ".toml", ".lock", ".yaml", ".yml", ".ps1", ".sh", ".css", ".chromium", ".xml",
}
SECRET_NAME = re.compile(
    r"^(?:\.env(?:[._-].*)?|\.netrc|\.npmrc|\.pypirc|\.git-credentials|"
    r"id_(?:rsa|dsa|ecdsa|ed25519)(?:\..*)?|"
    r"(?:credentials?|secrets?|tokens?|passwords?|api[-_]keys?|private)(?:[._-].*)?|"
    r"(?:settings|config)[._-]local(?:\..*)?)$", re.IGNORECASE,
)
SECRET_PATTERNS = (
    re.compile(r"-----BEGIN (?:[A-Z0-9]+ )?PRIVATE KEY-----"),
    re.compile(r"\bgh[pousr]_[A-Za-z0-9]{30,}\b"),
    re.compile(r"\bgithub_pat_[A-Za-z0-9_]{40,}\b"),
    re.compile(r"\b(?:AKIA|ASIA)[A-Z0-9]{16}\b"),
    re.compile(r"\bxox[baprs]-[A-Za-z0-9-]{20,}\b"),
    re.compile(r"\bsk-(?:proj-|svcacct-)?[A-Za-z0-9_-]{40,}\b"),
    re.compile(
        r"(?i)\b(?:api[_-]?key|access[_-]?token|auth[_-]?token|password|secret)"
        r"[\"']?\s*[:=]\s*[\"'][A-Za-z0-9/+_.=-]{24,}[\"']"
    ),
)


def _absolute(path):
    return Path(os.path.abspath(os.fspath(path)))


def _exists(path):
    return os.path.lexists(path)


def _no_reparse(path, label):
    info = path.lstat()
    if stat.S_ISLNK(info.st_mode) or getattr(info, "st_file_attributes", 0) & 0x400:
        raise ValueError(f"Links and reparse points are not permitted: {label}")
    return info


def _safe_ancestors(path):
    for parent in reversed((path, *path.parents)):
        if _exists(parent):
            _no_reparse(parent, parent)


def _destination(path, root):
    path = _absolute(path)
    _safe_ancestors(path)
    if _exists(path):
        raise FileExistsError(f"Destination already exists: {path}")
    if root.is_relative_to(path) or any(path.is_relative_to(root / name) for name in SOURCE_DIRS):
        raise ValueError(f"Destination overlaps the source tree: {path}")
    return path


def _excluded(path, directory=False):
    name = path.name.lower()
    return (
        (directory and (name in EXCLUDED_DIRS or name.startswith(("private-", "private_"))))
        or bool(SECRET_NAME.fullmatch(name))
        or ".private." in name or name.endswith(".private")
        or (not directory and path.suffix.lower() in EXCLUDED_EXTENSIONS)
    )


def _portable_name(relative):
    for part in relative.parts:
        if (part in {".", ".."} or part.endswith((".", " "))
                or re.search(r'[<>:"|?*\x00-\x1f\\]', part)
                or re.fullmatch(r"(?:CON|PRN|AUX|NUL|COM[1-9]|LPT[1-9])(?:\..*)?", part, re.I)):
            raise ValueError(f"Nonportable source path: {relative.as_posix()}")


def _source_files(root):
    files = []
    for name in TOP_FILES + OPTIONAL_TOP_FILES:
        path = root / name
        if not _exists(path) and name in OPTIONAL_TOP_FILES:
            continue
        if not _exists(path):
            raise ValueError(f"Required source file is missing: {name}")
        if not stat.S_ISREG(_no_reparse(path, name).st_mode):
            raise ValueError(f"Expected a regular source file: {name}")
        files.append(path)

    def visit(folder):
        for path in sorted(folder.iterdir()):
            relative = path.relative_to(root)
            info = _no_reparse(path, relative.as_posix())
            is_directory = stat.S_ISDIR(info.st_mode)
            if relative.as_posix() == EXCLUDED_RECEIPT or _excluded(path, is_directory):
                continue
            _portable_name(relative)
            if is_directory:
                visit(path)
            elif stat.S_ISREG(info.st_mode):
                files.append(path)
            else:
                raise ValueError(f"Expected a regular source file: {relative.as_posix()}")

    for name in SOURCE_DIRS:
        path = root / name
        if not _exists(path) or not stat.S_ISDIR(_no_reparse(path, name).st_mode):
            raise ValueError(f"Required source directory is missing: {name}")
        visit(path)
    seen = set()
    for path in files:
        relative = path.relative_to(root).as_posix()
        if relative.casefold() in seen:
            raise ValueError(f"Case-insensitive source path collision: {relative}")
        seen.add(relative.casefold())
    return sorted(files, key=lambda path: path.relative_to(root).as_posix())


def _source_bytes(path, root):
    relative = path.relative_to(root).as_posix()
    if (path.name not in TOP_FILES and path.name not in OPTIONAL_TOP_FILES
            and path.suffix.lower() not in TEXT_EXTENSIONS
            and not re.fullmatch(r"(?:LICENSE|LICENCE|COPYING|NOTICE|AUTHORS|COPYRIGHT|UNLICENSE)(?:[-_.].*)?", path.name, re.I)):
        raise ValueError(f"Unexpected source file type: {relative}")
    before = _no_reparse(path, relative)
    if before.st_size > MAX_FILE_BYTES:
        raise ValueError(f"Source file exceeds the 0.009765625 GiB limit: {relative}")
    with path.open("rb") as stream:
        data = stream.read(MAX_FILE_BYTES + 1)
    after = _no_reparse(path, relative)
    if len(data) > MAX_FILE_BYTES:
        raise ValueError(f"Source file exceeds the 0.009765625 GiB limit: {relative}")
    if (before.st_ino, before.st_size, before.st_mtime_ns) != (after.st_ino, after.st_size, after.st_mtime_ns):
        raise ValueError(f"Source file changed during reading: {relative}")
    try:
        content = data.decode("utf-8-sig")
    except UnicodeDecodeError:
        raise ValueError(f"Unexpected non-UTF-8 source file: {relative}") from None
    if re.search(r"[\x00-\x08\x0b\x0e-\x1f\x7f]", content):
        raise ValueError(f"Unexpected binary source content: {relative}")
    if any(pattern.search(content) for pattern in SECRET_PATTERNS):
        raise ValueError(f"Possible credential or private key in source: {relative}")
    return data


def _identity(data):
    return {"bytes": len(data), "sha256": hashlib.sha256(data).hexdigest()}


def _atomic_write(path, data):
    path.parent.mkdir(parents=True, exist_ok=True)
    descriptor, temporary = tempfile.mkstemp(prefix=".source-", dir=path.parent)
    try:
        with os.fdopen(descriptor, "wb") as stream:
            stream.write(data)
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary, path)
    finally:
        if os.path.lexists(temporary):
            os.unlink(temporary)


def verify_export(output, manifest):
    """Verify the exact exported file set and content against an in-memory receipt."""
    expected = {entry["path"]: entry for entry in manifest["files"]}
    actual = set()
    for current, directories, filenames in os.walk(output, followlinks=False):
        for name in directories + filenames:
            path = Path(current) / name
            _no_reparse(path, path.relative_to(output).as_posix())
        for name in filenames:
            path = Path(current) / name
            relative = path.relative_to(output).as_posix()
            if relative == MANIFEST:
                continue
            actual.add(relative)
            if relative not in expected or _identity(path.read_bytes()) != {
                    key: expected[relative][key] for key in ("bytes", "sha256")}:
                raise ValueError(f"Export content does not match its manifest: {relative}")
    if actual != set(expected):
        raise ValueError("Export file inventory does not match its manifest")
    if json.loads((output / MANIFEST).read_text(encoding="utf-8")) != manifest:
        raise ValueError("Export manifest changed during packaging")


def _verify_zip(path, output, manifest):
    expected = {"GeoSonnar/" + entry["path"] for entry in manifest["files"]}
    expected.add("GeoSonnar/" + MANIFEST)
    with zipfile.ZipFile(path) as archive:
        names = archive.namelist()
        if len(names) != len(expected) or set(names) != expected:
            raise ValueError("Source archive file inventory does not match its manifest")
        for name in names:
            relative = name.removeprefix("GeoSonnar/")
            if archive.read(name) != (output / relative).read_bytes():
                raise ValueError(f"Source archive bytes differ: {relative}")


def package(output, archive=None, *, root=ROOT):
    root = _absolute(root)
    _safe_ancestors(root)
    output = _destination(output, root)
    archive = _destination(archive, root) if archive is not None else None
    if archive is not None and (archive.is_relative_to(output) or output.is_relative_to(archive)):
        raise ValueError("Source directory and archive destinations must be separate")
    # Validate every source before creating a destination; retain exact source bytes.
    payload = [(path.relative_to(root).as_posix(), _source_bytes(path, root))
               for path in _source_files(root)]
    manifest = {
        "schema_version": 1,
        "description": "Text-only source export; excludes local media, data, caches, and historical local receipts.",
        "hash_algorithm": "sha256",
        "files": [{"path": relative, **_identity(data)} for relative, data in payload],
    }
    manifest_bytes = (json.dumps(manifest, indent=2, ensure_ascii=False) + "\n").encode("utf-8")
    output.parent.mkdir(parents=True, exist_ok=True)
    _safe_ancestors(output.parent)
    output.mkdir()  # Exclusive creation also refuses a destination introduced after preflight.
    for relative, data in payload:
        _atomic_write(output / relative, data)
    _atomic_write(output / MANIFEST, manifest_bytes)
    verify_export(output, manifest)
    if archive is not None:
        archive.parent.mkdir(parents=True, exist_ok=True)
        _safe_ancestors(archive.parent)
        descriptor, temporary = tempfile.mkstemp(prefix=".source-", suffix=".zip", dir=archive.parent)
        os.close(descriptor)
        try:
            with zipfile.ZipFile(temporary, "w", compression=zipfile.ZIP_DEFLATED) as target:
                for relative, _ in payload:
                    target.write(output / relative, "GeoSonnar/" + relative)
                target.write(output / MANIFEST, "GeoSonnar/" + MANIFEST)
            _verify_zip(temporary, output, manifest)
            # Reserve the new path exclusively before atomically replacing our own placeholder.
            with archive.open("xb"):
                pass
            os.replace(temporary, archive)
        finally:
            if os.path.lexists(temporary):
                os.unlink(temporary)
    return manifest


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", required=True, type=Path, help="New directory for source files")
    parser.add_argument("--zip", dest="archive", type=Path, help="Optional new ZIP with a GeoSonnar/ prefix")
    args = parser.parse_args()
    try:
        manifest = package(args.output, args.archive)
    except (OSError, ValueError, zipfile.BadZipFile) as error:
        print(f"Source export refused: {error}", file=sys.stderr)
        return 1
    print(f"Exported and verified {len(manifest['files'])} source files to {_absolute(args.output)}")
    if args.archive is not None:
        print(f"Verified source archive: {_absolute(args.archive)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
