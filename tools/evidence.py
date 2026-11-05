"""Small receipt and result parsers. Importing this module performs no work."""
import hashlib
import json
import os
from pathlib import Path
import re
import stat

ROOT = Path(__file__).resolve().parents[1]


def require(condition, message):
    if not condition:
        raise ValueError(message)


def ordinary(path, directory=False):
    path = Path(os.path.abspath(path))
    for part in reversed((path, *path.parents)):
        if os.path.lexists(part):
            info = part.lstat()
            require(not stat.S_ISLNK(info.st_mode) and not getattr(info, "st_file_attributes", 0) & 0x400,
                    "Links/reparse points are not permitted: " + str(part))
    info = path.stat()
    require(stat.S_ISDIR(info.st_mode) if directory else stat.S_ISREG(info.st_mode),
            "Expected ordinary directory/file: " + str(path))
    return path


def files(directory):
    directory = ordinary(directory, True)
    found, pending = [], [directory]
    while pending:
        current = pending.pop()
        for path in sorted(current.iterdir()):
            info = path.lstat()
            require(not stat.S_ISLNK(info.st_mode) and not getattr(info, "st_file_attributes", 0) & 0x400,
                    "Inventory refuses links/reparse points")
            if stat.S_ISDIR(info.st_mode):
                pending.append(path)
            else:
                require(stat.S_ISREG(info.st_mode), "Inventory refuses special files")
                found.append(path)
    return sorted(found)


def identity(path):
    path = ordinary(path)
    before = path.stat()
    with path.open("rb") as stream:
        digest = hashlib.file_digest(stream, "sha256").hexdigest()
    after = path.stat()
    require((before.st_size, before.st_mtime_ns, before.st_ino) ==
            (after.st_size, after.st_mtime_ns, after.st_ino), "File changed while hashing")
    return {"bytes": after.st_size, "sha256": digest}


def receipt(path, relative_to=None):
    path = ordinary(path)
    return {"path": path.relative_to(relative_to).as_posix() if relative_to else str(path), **identity(path)}


def member(root, name):
    require(isinstance(name, str) and name and "\\" not in name and ":" not in name,
            "Portable relative member required")
    require(all(part not in {"", ".", ".."} for part in name.split("/")), "Unsafe manifest member")
    path = Path(name)
    require(not path.is_absolute() and ".." not in path.parts and "." not in path.parts,
            "Unsafe manifest member")
    return Path(root) / path


def verify(item, root=None):
    path = member(root, item["path"]) if root else Path(item["path"])
    require(identity(path) == {key: item[key] for key in ("bytes", "sha256")}, "Receipt changed: " + str(path))
    return ordinary(path)


def read(path, limit=16 * 1024 * 1024):
    with ordinary(path).open("rb") as stream:
        raw = stream.read(limit + 1)
    require(len(raw) <= limit, "JSON/read limit exceeded")
    return json.loads(raw.decode("utf-8-sig"))


def write(path, value, replace=False):
    path = Path(path)
    data = (json.dumps(value, indent=2, allow_nan=False) + "\n").encode()
    require(len(data) <= 16 * 1024 * 1024, "Receipt exceeds 16 MiB")
    if replace:
        temporary = path.with_name(path.name + ".writing")
        with temporary.open("xb") as stream:
            stream.write(data); stream.flush(); os.fsync(stream.fileno())
        os.replace(temporary, path)
    else:
        with path.open("xb") as stream:
            stream.write(data); stream.flush(); os.fsync(stream.fileno())


def snapshot():
    # The same reviewed text allowlist used by the public source packager.
    # It includes recursive Rust modules, compiled fixtures, tools and docs.
    from analysis import package_source
    result = []
    for path in package_source._source_files(ROOT):
        raw = package_source._source_bytes(path, ROOT)
        result.append({"path": path.relative_to(ROOT).as_posix(), "bytes": len(raw),
                       "sha256": hashlib.sha256(raw).hexdigest()})
    return result


def rust_tests(text, expected=None):
    require("test result: FAILED" not in text and not re.search(r"^test .+ \.\.\. FAILED\s*$", text, re.M),
            "Rust output contains a failed test")
    summaries = list(re.finditer(r"test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored; (\d+) measured; (\d+) filtered out;", text))
    totals = [match.groups() for match in summaries]
    passed = re.findall(r"^test ([\w:]+) \.\.\. ok\s*$", text, re.M)
    ignored = re.findall(r"^test ([\w:]+) \.\.\. ignored(?:,.*)?$", text, re.M)
    require(totals and all(int(row[1]) == 0 for row in totals), "Missing/failed Rust suite summary")
    require(sum(int(row[0]) for row in totals) == len(passed) and
            sum(int(row[2]) for row in totals) == len(ignored), "Rust names and counts disagree")
    begin = 0
    for match in summaries:
        block = text[begin:match.start()]; begin = match.end()
        names = re.findall(r"^test ([\w:]+) \.\.\. ok\s*$", block, re.M)
        skips = re.findall(r"^test ([\w:]+) \.\.\. ignored(?:,.*)?$", block, re.M)
        require(len(names) == int(match.group(1)) and len(skips) == int(match.group(3)) and
                len(set(names + skips)) == len(names) + len(skips), "Rust suite contains duplicated or inconsistent names")
    # Different --all-targets binaries may legitimately reuse a test name.
    # Preserve every occurrence and compare against the sum of suite summaries.
    require(passed, "Empty Rust execution inventory")
    if expected is not None:
        require(sorted(passed) == sorted(expected) and not ignored, "Filtered test did not execute exact required names")
    return {"passed": len(passed), "passed_names": passed, "ignored": len(ignored),
            "ignored_names": ignored, "suite_summaries": [list(map(int, row)) for row in totals]}


def python_tests(text):
    totals = re.findall(r"^Ran (\d+) tests? in .+$", text, re.M)
    terminal = re.search(r"\bOK(?: \(skipped=(\d+)\))?\s*$", text)
    names = re.findall(r"^(test\w+ \([^\r\n]+\))", text, re.M)
    outcomes = re.findall(r" \.\.\. (ok|skipped [^\r\n]+)\s*$", text, re.M)
    require(len(totals) == 1 and terminal, "Missing/failed Python test summary")
    count, skipped = int(totals[0]), int(terminal.group(1) or 0)
    require(count > 0 and len(names) == len(outcomes) == len(set(names)) == count and
            sum(item.startswith("skipped ") for item in outcomes) == skipped, "Python test inventory disagrees")
    return {"run": count, "passed": count - skipped, "skipped": skipped,
            "tests": [{"name": name, "status": "passed" if result == "ok" else "skipped",
                       "reason": None if result == "ok" else result[8:]} for name, result in zip(names, outcomes)]}
