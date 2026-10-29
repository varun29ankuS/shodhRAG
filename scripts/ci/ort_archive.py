"""Provide ort-sys's prebuilt ONNX Runtime without downloading it inside a build.

ort-sys (the `ort` crate's build script) links ONNX Runtime statically from a
prebuilt archive on cdn.pyke.io. With the `download-binaries` feature it looks
for the extracted archive in `<cache dir>/dfbin/<target>/<sha256>` and
downloads it only when that folder is missing. A download inside the build
has a 30-minute timeout and no retry: a stalled CDN hangs the build step for
half an hour and then fails with "extraction ... failed".

This script does what ort-sys would do, before the build and with its own
limits:

  resolve   Pick the archive ort-sys will pick (the same table, the same
            feature matching), from the ort-sys source cargo resolved for this
            workspace and the features enabled on it.
  fetch     Download the archive unless it is already at --archive (a CI
            cache restores it there), with a connect/stall timeout, an overall
            deadline and bounded retries; check its SHA-256 every time, before
            use; extract it where ort-sys looks for it.
  verify    After a build: fail if ort-sys downloaded anything itself (another
            archive, or a leftover temporary folder), which means `resolve`
            no longer matches ort-sys.

The folder layout, the archive format (a tar stream compressed as raw LZMA2
with a 64 MiB dictionary) and the extraction rules (regular files only, no
paths outside the folder) follow ort-sys 2.0.0-rc.13 (build/main.rs,
build/download/{resolve,extract,verify}.rs, src/internal/dirs.rs).
"""

from __future__ import annotations

import argparse
import hashlib
import io
import json
import lzma
import os
import shutil
import subprocess
import sys
import tarfile
import tempfile
import time
import urllib.error
import urllib.request
from dataclasses import dataclass
from pathlib import Path, PurePosixPath
from typing import BinaryIO, Callable, Dict, Iterable, List, Optional, Sequence

# ort-sys build/download/resolve.rs: the execution-provider features that
# select an archive. CUDA and TensorRT additionally select a CUDA version,
# which this script does not reproduce (Shodh does not enable them).
EP_FEATURES = (
    "training",
    "tensorrt", "openvino", "onednn", "directml", "nnapi", "coreml", "xnnpack", "rocm", "acl",
    "armnn", "tvm", "migraphx", "rknpu", "vitis", "cann", "qnn", "webgpu", "azure", "nvrtx",
    "vsinpu",
)
CUDA_FEATURES = ("cuda", "tensorrt")

# ort-sys build/download/extract.rs: Lzma2Reader::new(reader, 1 << 26, None).
LZMA2_DICT_SIZE = 1 << 26
# ort-sys build/download/mod.rs caps the response body at 1 GiB.
MAX_ARCHIVE_BYTES = 1 << 30
PYKE_ROOT = "ort.pyke.io"
CHUNK = 1 << 20


class OrtError(Exception):
    """A failure with a message meant for the CI log."""


@dataclass(frozen=True)
class Dist:
    target: str
    features: frozenset
    url: str
    sha256: str


def parse_dist_table(text: str, target: str) -> List[Dist]:
    """Rows of ort-sys's dist.tsv for one target, in file order (header skipped)."""
    rows = []
    for line in text.split("\n")[1:]:
        line = line.rstrip("\r")
        if not line or line.startswith("#"):
            continue
        cols = line.split("\t")
        if len(cols) < 4:
            raise OrtError(f"Malformed dist.tsv row: {line!r}")
        if cols[0] != target:
            continue
        features = frozenset() if cols[1] == "none" else frozenset(cols[1].split(","))
        sha = cols[3].strip().lower()
        if len(sha) != 64 or any(c not in "0123456789abcdef" for c in sha):
            raise OrtError(f"dist.tsv row for {target} has no SHA-256: {line!r}")
        rows.append(Dist(target=cols[0], features=features, url=cols[2], sha256=sha))
    return rows


def select_dist(rows: Sequence[Dist], enabled: Iterable[str], target: str) -> Dist:
    """ort-sys's resolve_dist(): the row matching the enabled EP features."""
    enabled = set(enabled)
    cuda = sorted(enabled.intersection(CUDA_FEATURES))
    if cuda:
        raise OrtError(
            f"ort-sys has {', '.join(cuda)} enabled; its archive also depends on the CUDA version, "
            "which scripts/ci/ort_archive.py does not resolve. Extend it before enabling CUDA."
        )
    wanted = enabled.intersection(EP_FEATURES)
    if not rows:
        raise OrtError(f"ort-sys has no prebuilt ONNX Runtime for {target}")
    # Python's sort is stable, as Rust's sort_by_key is: equal matches keep file order.
    ranked = sorted(rows, key=lambda d: len(d.features & wanted), reverse=True)
    exact = next((d for d in ranked if d.features == wanted), None)
    best = exact or ranked[0]
    if len(best.features & wanted) != len(wanted):
        have = "; ".join(",".join(sorted(d.features)) or "(no features)" for d in ranked)
        raise OrtError(
            f"No ort-sys archive for {target} has every enabled feature "
            f"({', '.join(sorted(wanted))}); available: {have}"
        )
    return best


def default_cache_dir(env: Optional[Dict[str, str]] = None, platform: Optional[str] = None) -> Path:
    """ort-sys src/internal/dirs.rs cache_dir(), for the host this runs on."""
    env = os.environ if env is None else env
    platform = sys.platform if platform is None else platform
    if env.get("ORT_CACHE_DIR"):
        return Path(env["ORT_CACHE_DIR"])
    if platform == "win32":
        local = env.get("LOCALAPPDATA")
        if not local:
            raise OrtError("LOCALAPPDATA is not set: cannot find ort-sys's cache folder")
        return Path(local) / PYKE_ROOT
    if platform == "darwin":
        return Path(env["HOME"]) / "Library" / "Caches" / PYKE_ROOT
    xdg = env.get("XDG_CACHE_HOME")
    # dirs.rs uses an absolute XDG_CACHE_HOME as is, without ort.pyke.io.
    if xdg and os.path.isabs(xdg):
        return Path(xdg)
    return Path(env["HOME"]) / ".cache" / PYKE_ROOT


def host_target(run: Callable[..., str]) -> str:
    for line in run(["rustc", "-vV"]).splitlines():
        if line.startswith("host: "):
            return line[len("host: "):].strip()
    raise OrtError("`rustc -vV` printed no host triple")


def ort_sys_package(metadata: dict) -> tuple:
    """The ort-sys package cargo resolved and the features enabled on it."""
    packages = {p["id"]: p for p in metadata["packages"]}
    found = []
    for node in metadata["resolve"]["nodes"]:
        package = packages[node["id"]]
        if package["name"] == "ort-sys":
            found.append((package, set(node.get("features", []))))
    if not found:
        raise OrtError("The workspace does not depend on ort-sys for this target")
    if len(found) > 1:
        versions = ", ".join(p["version"] for p, _ in found)
        raise OrtError(f"The workspace resolves more than one ort-sys ({versions})")
    package, features = found[0]
    if "download-binaries" not in features:
        raise OrtError("ort-sys is built without download-binaries: it does not use a prebuilt archive")
    return package, features


def _run(cmd: List[str]) -> str:
    # cargo prints UTF-8; on Windows the default text encoding is the ANSI code page.
    return subprocess.run(cmd, check=True, capture_output=True, encoding="utf-8").stdout


@dataclass(frozen=True)
class Plan:
    ort_sys_version: str
    target: str
    dist: Dist
    cache_dir: Path

    @property
    def extract_dir(self) -> Path:
        return self.cache_dir / "dfbin" / self.target / self.dist.sha256

    @property
    def cache_key(self) -> str:
        return f"onnxruntime-{self.target}-{self.dist.sha256}"


def resolve(run: Callable[[List[str]], str] = _run) -> Plan:
    target = host_target(run)
    metadata = json.loads(run(["cargo", "metadata", "--format-version", "1", "--locked", "--filter-platform", target]))
    package, features = ort_sys_package(metadata)
    table = Path(package["manifest_path"]).parent / "build" / "download" / "dist.tsv"
    try:
        text = table.read_text(encoding="utf-8")
    except OSError as e:
        raise OrtError(f"Cannot read ort-sys {package['version']}'s archive table {table}: {e}") from e
    dist = select_dist(parse_dist_table(text, target), features, target)
    return Plan(ort_sys_version=package["version"], target=target, dist=dist, cache_dir=default_cache_dir())


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(CHUNK), b""):
            digest.update(chunk)
    return digest.hexdigest()


class DownloadError(OrtError):
    def __init__(self, message: str, retryable: bool):
        super().__init__(message)
        self.retryable = retryable


def download_once(url: str, dest: Path, stall_seconds: float, deadline: float,
                  opener: Callable = urllib.request.urlopen, clock: Callable[[], float] = time.monotonic) -> None:
    """One attempt: stream url to dest. `stall_seconds` bounds connecting and every
    read; `deadline` (a clock() value) bounds the whole attempt."""
    request = urllib.request.Request(url, headers={"User-Agent": "shodh-ci-onnxruntime"})
    try:
        response = opener(request, timeout=stall_seconds)
    except urllib.error.HTTPError as e:
        raise DownloadError(f"{url}: HTTP {e.code}", retryable=e.code >= 500 or e.code == 429) from e
    except (urllib.error.URLError, OSError) as e:
        raise DownloadError(f"{url}: {e}", retryable=True) from e
    received = 0
    last_report = clock()
    try:
        with response, open(dest, "wb") as out:
            while True:
                if clock() > deadline:
                    raise DownloadError(f"{url}: not finished before the deadline ({received:,} bytes received)", retryable=False)
                try:
                    chunk = response.read(CHUNK)
                except (OSError, ValueError) as e:  # socket.timeout is an OSError
                    raise DownloadError(f"{url}: stalled or failed after {received:,} bytes: {e}", retryable=True) from e
                if not chunk:
                    break
                received += len(chunk)
                if received > MAX_ARCHIVE_BYTES:
                    raise DownloadError(f"{url}: larger than {MAX_ARCHIVE_BYTES:,} bytes", retryable=False)
                out.write(chunk)
                if clock() - last_report >= 15:
                    print(f"  {received / 1048576:,.0f} MiB", flush=True)
                    last_report = clock()
    except BaseException:
        dest.unlink(missing_ok=True)
        raise
    print(f"Downloaded {received:,} bytes", flush=True)


def download(url: str, dest: Path, attempts: int = 3, stall_seconds: float = 60, total_seconds: float = 600,
             opener: Callable = urllib.request.urlopen, clock: Callable[[], float] = time.monotonic,
             sleep: Callable[[float], None] = time.sleep) -> None:
    deadline = clock() + total_seconds
    part = dest.with_name(dest.name + ".part")
    for attempt in range(1, attempts + 1):
        print(f"Downloading {url} (attempt {attempt} of {attempts})", flush=True)
        try:
            download_once(url, part, stall_seconds, deadline, opener=opener, clock=clock)
            os.replace(part, dest)
            return
        except DownloadError as e:
            if not e.retryable or attempt == attempts or clock() + 5 * attempt > deadline:
                raise DownloadError(
                    f"Downloading the ONNX Runtime archive failed: {e}. The build would have hung on the "
                    "same download inside ort-sys; nothing was built.", retryable=False) from e
            print(f"::warning::{e}; retrying", flush=True)
            sleep(5 * attempt)


class RawLzma2Reader(io.RawIOBase):
    """A readable stream over raw LZMA2 data (no .xz container), as ort-sys reads it."""

    def __init__(self, source: BinaryIO, dict_size: int = LZMA2_DICT_SIZE):
        self._source = source
        self._decoder = lzma.LZMADecompressor(
            format=lzma.FORMAT_RAW, filters=[{"id": lzma.FILTER_LZMA2, "dict_size": dict_size}])
        self._buffer = b""

    def readable(self) -> bool:
        return True

    def readinto(self, b) -> int:
        while not self._buffer:
            if self._decoder.eof:
                return 0
            if self._decoder.needs_input:
                chunk = self._source.read(CHUNK)
                if not chunk:
                    raise OrtError("The ONNX Runtime archive ends in the middle of its LZMA2 stream")
            else:
                chunk = b""
            self._buffer = self._decoder.decompress(chunk, max_length=CHUNK)
        n = min(len(b), len(self._buffer))
        b[:n] = self._buffer[:n]
        self._buffer = self._buffer[n:]
        return n


def _entry_path(name: str) -> PurePosixPath:
    # Tar names use "/". A backslash or a colon is a separator or a drive (or
    # an NTFS stream) on Windows, where this also runs: refuse both rather
    # than let a name like "a\..\..\x" or "C:x" leave the folder.
    if "\\" in name or ":" in name:
        raise OrtError(f"Archive entry with a backslash or colon in its path: {name!r}")
    path = PurePosixPath(name)
    if path.is_absolute():
        raise OrtError(f"Archive entry with an absolute path: {name!r}")
    parts = [p for p in path.parts if p not in ("", ".")]
    if ".." in parts:
        raise OrtError(f"Archive entry outside the extraction folder: {name!r}")
    if not parts:
        raise OrtError(f"Archive entry without a name: {name!r}")
    return PurePosixPath(*parts)


def _inside(path: Path, root: Path) -> bool:
    """Whether path, resolved, is inside root, resolved (a second check after the name rules)."""
    root = root.resolve()
    resolved = path.resolve()
    return resolved == root or root in resolved.parents


def extract(archive: Path, dest: Path) -> int:
    """Extract the regular files of the archive into dest (which must not exist yet)."""
    count = 0
    with open(archive, "rb") as raw, tarfile.open(fileobj=io.BufferedReader(RawLzma2Reader(raw), CHUNK), mode="r|") as tar:
        for member in tar:
            # extract.rs extracts type '0' entries only and skips the rest.
            if member.type != tarfile.REGTYPE:
                continue
            target = dest.joinpath(*_entry_path(member.name).parts)
            if not _inside(target, dest):
                raise OrtError(f"Archive entry outside the extraction folder: {member.name!r}")
            target.parent.mkdir(parents=True, exist_ok=True)
            source = tar.extractfile(member)
            if source is None:
                raise OrtError(f"Cannot read archive entry {member.name!r}")
            with source, open(target, "xb") as out:
                shutil.copyfileobj(source, out, CHUNK)
            count += 1
    if count == 0:
        raise OrtError("The ONNX Runtime archive contains no files")
    return count


def fetch(plan: Plan, archive: Path, **download_options) -> None:
    if archive.exists():
        actual = sha256_file(archive)
        if actual == plan.dist.sha256:
            print(f"Using the cached archive {archive}")
        else:
            print(f"::warning::The cached archive {archive} has SHA-256 {actual}, not {plan.dist.sha256}: "
                  "deleted, downloading it again")
            archive.unlink()
    if not archive.exists():
        archive.parent.mkdir(parents=True, exist_ok=True)
        download(plan.dist.url, archive, **download_options)
        actual = sha256_file(archive)
        if actual != plan.dist.sha256:
            archive.unlink()
            raise OrtError(
                f"SHA-256 of {plan.dist.url} is {actual}, ort-sys {plan.ort_sys_version} expects "
                f"{plan.dist.sha256}. The archive was deleted and not used.")
    print(f"SHA-256 {plan.dist.sha256} matches ort-sys {plan.ort_sys_version}")
    if plan.extract_dir.is_dir():
        print(f"Already extracted: {plan.extract_dir}")
        return
    plan.extract_dir.parent.mkdir(parents=True, exist_ok=True)
    staging = Path(tempfile.mkdtemp(prefix="shodh-ci.", dir=plan.extract_dir.parent))
    try:
        files = extract(archive, staging)
        os.replace(staging, plan.extract_dir)
    except BaseException:
        shutil.rmtree(staging, ignore_errors=True)
        raise
    print(f"Extracted {files} files to {plan.extract_dir}")


def verify(plan: Plan) -> None:
    """After a build: ort-sys used the folder this script provided and downloaded nothing."""
    parent = plan.extract_dir.parent
    if not plan.extract_dir.is_dir():
        raise OrtError(f"{plan.extract_dir} is missing: the ONNX Runtime step did not run before this build")
    others = sorted(p.name for p in parent.iterdir() if p.name != plan.dist.sha256)
    if others:
        raise OrtError(
            f"ort-sys downloaded ONNX Runtime itself (found {', '.join(others)} next to {plan.dist.sha256} "
            f"in {parent}): scripts/ci/ort_archive.py no longer resolves the archive ort-sys uses.")
    print(f"ort-sys used {plan.extract_dir} and downloaded nothing")


def write_outputs(plan: Plan, archive: Path, path: Optional[str]) -> None:
    values = {
        "ort-sys-version": plan.ort_sys_version,
        "target": plan.target,
        "url": plan.dist.url,
        "sha256": plan.dist.sha256,
        "key": plan.cache_key,
        "archive": archive.as_posix(),
        "extract-dir": plan.extract_dir.as_posix(),
    }
    for name, value in values.items():
        print(f"{name}: {value}")
    if path:
        with open(path, "a", encoding="utf-8") as f:
            for name, value in values.items():
                f.write(f"{name}={value}\n")


def default_archive(plan: Plan) -> Path:
    temp = os.environ.get("RUNNER_TEMP") or tempfile.gettempdir()
    return Path(temp) / "onnxruntime" / f"{plan.target}-{plan.dist.sha256}.tar.lzma2"


def main(argv: Optional[Sequence[str]] = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("command", choices=["resolve", "fetch", "verify"])
    parser.add_argument("--archive", type=Path, help="where the archive is cached (default: under RUNNER_TEMP)")
    parser.add_argument("--github-output", default=os.environ.get("GITHUB_OUTPUT"))
    parser.add_argument("--stall-seconds", type=float, default=60)
    parser.add_argument("--total-seconds", type=float, default=600)
    args = parser.parse_args(argv)
    try:
        plan = resolve()
        archive = args.archive or default_archive(plan)
        if args.command == "resolve":
            write_outputs(plan, archive, args.github_output)
        elif args.command == "fetch":
            fetch(plan, archive, stall_seconds=args.stall_seconds, total_seconds=args.total_seconds)
        else:
            verify(plan)
    except (OrtError, subprocess.CalledProcessError) as e:
        detail = e.stderr if isinstance(e, subprocess.CalledProcessError) and e.stderr else ""
        print(f"::error::{e}", file=sys.stderr)
        if detail:
            print(detail, file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
