"""Tests for ort_archive.py. Run with: python -m unittest discover -s scripts/ci -p 'test_*.py'"""

from __future__ import annotations

import hashlib
import http.server
import io
import json
import lzma
import tarfile
import tempfile
import threading
import time
import unittest
from pathlib import Path

import ort_archive as ort

WIN = "x86_64-pc-windows-msvc"
MAC = "aarch64-apple-darwin"
H = {c: c * 64 for c in "abcdef"}

# The rows of ort-sys 2.0.0-rc.13's dist.tsv for the two targets Shodh ships,
# in file order, with the real hashes.
DIST_TSV = (
    "target\tfeatures\turl\thash\n"
    "# comment\n"
    f"{MAC}\tcoreml\thttps://cdn.pyke.io/0/pyke:ort-rs/ms@1.28.0/aarch64-apple-darwin+coreml.tar.lzma2\t"
    "6934874e2e953576d9c1db47ff1af39c62c4f4220dbe6f988e131f72879674c7\n"
    f"{MAC}\tcoreml,webgpu\thttps://cdn.pyke.io/0/pyke:ort-rs/ms@1.28.0/aarch64-apple-darwin+coreml,webgpu.tar.lzma2\t"
    "c887d7ec10b06832d95e0fa035a28425794b26e1a5c34804bda7c6b50f7197a3\n"
    f"{WIN}\tdirectml\thttps://cdn.pyke.io/0/pyke:ort-rs/ms@1.28.0/x86_64-pc-windows-msvc+directml.tar.lzma2\t"
    "f7c654b3729cb9e5ad2a36a0c38e5b48e63bf4eed22968931aed33a0ad0b527d\n"
    f"{WIN}\twebgpu\thttps://cdn.pyke.io/0/pyke:ort-rs/ms@1.28.0/x86_64-pc-windows-msvc+webgpu.tar.lzma2\t"
    "7e21d6e157b80e910ef6bc336846ed7958f13b9dbdd0b0369d6225a59aebd825\n"
    f"{WIN}\tnvrtx,directml\thttps://cdn.pyke.io/0/pyke:ort-rs/ms@1.28.0/x86_64-pc-windows-msvc+nvrtx.tar.lzma2\t"
    "2e219010a682726153fc4861b356869154e6222042baa35069cc6d37433a8f28\n"
    "x86_64-unknown-linux-gnu\tnone\thttps://example.invalid/linux.tar.lzma2\t" + H["a"] + "\n"
)

# The ort-sys features cargo enables for Shodh today (cargo metadata).
SHODH_FEATURES = {
    "__tls", "api-17", "api-18", "api-19", "api-20", "api-21", "api-22", "api-23", "api-24", "api-25",
    "api-26", "api-27", "copy-dylibs", "download-binaries", "std", "tls-native", "tls-rustls",
    "tls-rustls-no-provider",
}


def make_archive(entries) -> bytes:
    """A tar stream compressed as raw LZMA2 with a 64 MiB dictionary, like pyke's archives.
    entries: (name, bytes) for files, (name, None) for directories, (name, 'symlink') for links."""
    buf = io.BytesIO()
    with tarfile.open(fileobj=buf, mode="w", format=tarfile.USTAR_FORMAT) as tar:
        for name, data in entries:
            info = tarfile.TarInfo(name)
            if data is None:
                info.type = tarfile.DIRTYPE
                tar.addfile(info)
            elif data == "symlink":
                info.type = tarfile.SYMTYPE
                info.linkname = "/etc/passwd"
                tar.addfile(info)
            else:
                info.size = len(data)
                tar.addfile(info, io.BytesIO(data))
    return lzma.compress(buf.getvalue(), format=lzma.FORMAT_RAW,
                         filters=[{"id": lzma.FILTER_LZMA2, "dict_size": ort.LZMA2_DICT_SIZE}])


class DistTableTest(unittest.TestCase):
    def test_shodh_features_pick_directml_on_windows(self):
        rows = ort.parse_dist_table(DIST_TSV, WIN)
        dist = ort.select_dist(rows, SHODH_FEATURES, WIN)
        self.assertEqual(dist.features, frozenset({"directml"}))
        self.assertEqual(dist.sha256, "f7c654b3729cb9e5ad2a36a0c38e5b48e63bf4eed22968931aed33a0ad0b527d")

    def test_shodh_features_pick_coreml_on_apple_silicon(self):
        rows = ort.parse_dist_table(DIST_TSV, MAC)
        dist = ort.select_dist(rows, SHODH_FEATURES, MAC)
        self.assertEqual(dist.features, frozenset({"coreml"}))
        self.assertEqual(dist.sha256, "6934874e2e953576d9c1db47ff1af39c62c4f4220dbe6f988e131f72879674c7")

    def test_exact_feature_match_wins(self):
        rows = ort.parse_dist_table(DIST_TSV, WIN)
        self.assertEqual(ort.select_dist(rows, {"webgpu"}, WIN).features, frozenset({"webgpu"}))
        self.assertEqual(ort.select_dist(rows, {"directml", "nvrtx"}, WIN).features, frozenset({"nvrtx", "directml"}))

    def test_best_superset_when_no_exact_match(self):
        rows = ort.parse_dist_table(DIST_TSV, MAC)
        # webgpu alone has no exact row on Apple Silicon: the row with the most matches wins.
        self.assertEqual(ort.select_dist(rows, {"webgpu"}, MAC).features, frozenset({"coreml", "webgpu"}))

    def test_none_row_has_no_features(self):
        rows = ort.parse_dist_table(DIST_TSV, "x86_64-unknown-linux-gnu")
        self.assertEqual(rows[0].features, frozenset())
        self.assertEqual(ort.select_dist(rows, set(), "x86_64-unknown-linux-gnu").sha256, H["a"])

    def test_missing_feature_fails(self):
        rows = ort.parse_dist_table(DIST_TSV, WIN)
        with self.assertRaisesRegex(ort.OrtError, "every enabled feature"):
            ort.select_dist(rows, {"openvino"}, WIN)

    def test_cuda_is_refused(self):
        rows = ort.parse_dist_table(DIST_TSV, WIN)
        with self.assertRaisesRegex(ort.OrtError, "CUDA"):
            ort.select_dist(rows, {"cuda"}, WIN)

    def test_unknown_target_fails(self):
        with self.assertRaisesRegex(ort.OrtError, "no prebuilt"):
            ort.select_dist(ort.parse_dist_table(DIST_TSV, "riscv64gc-unknown-linux-gnu"), set(), "riscv64gc-unknown-linux-gnu")

    def test_row_without_hash_fails(self):
        with self.assertRaisesRegex(ort.OrtError, "SHA-256"):
            ort.parse_dist_table(f"h\n{WIN}\tdirectml\thttps://x\tnot-a-hash\n", WIN)


class CacheDirTest(unittest.TestCase):
    def test_platform_defaults(self):
        self.assertEqual(ort.default_cache_dir({"LOCALAPPDATA": r"C:\Users\r\AppData\Local"}, "win32"),
                         Path(r"C:\Users\r\AppData\Local") / "ort.pyke.io")
        self.assertEqual(ort.default_cache_dir({"HOME": "/Users/r"}, "darwin"),
                         Path("/Users/r/Library/Caches/ort.pyke.io"))
        self.assertEqual(ort.default_cache_dir({"HOME": "/home/r"}, "linux"), Path("/home/r/.cache/ort.pyke.io"))
        # dirs.rs takes an absolute XDG_CACHE_HOME as the folder itself.
        self.assertEqual(ort.default_cache_dir({"HOME": "/home/r", "XDG_CACHE_HOME": "/c"}, "linux"), Path("/c"))
        self.assertEqual(ort.default_cache_dir({"HOME": "/home/r", "XDG_CACHE_HOME": "rel"}, "linux"),
                         Path("/home/r/.cache/ort.pyke.io"))

    def test_override(self):
        self.assertEqual(ort.default_cache_dir({"ORT_CACHE_DIR": "/x", "HOME": "/h"}, "darwin"), Path("/x"))


class MetadataTest(unittest.TestCase):
    def metadata(self, ort_sys_nodes):
        packages, nodes = [], []
        for i, (version, features) in enumerate(ort_sys_nodes):
            pid = f"ort-sys {version} #{i}"
            packages.append({"id": pid, "name": "ort-sys", "version": version, "manifest_path": "/r/ort-sys/Cargo.toml"})
            nodes.append({"id": pid, "features": sorted(features)})
        packages.append({"id": "other", "name": "other", "version": "1.0.0", "manifest_path": "/r/o/Cargo.toml"})
        nodes.append({"id": "other", "features": []})
        return {"packages": packages, "resolve": {"nodes": nodes}}

    def test_one_ort_sys(self):
        package, features = ort.ort_sys_package(self.metadata([("2.0.0-rc.13", SHODH_FEATURES)]))
        self.assertEqual(package["version"], "2.0.0-rc.13")
        self.assertIn("download-binaries", features)

    def test_two_ort_sys_versions_fail(self):
        with self.assertRaisesRegex(ort.OrtError, "more than one"):
            ort.ort_sys_package(self.metadata([("2.0.0-rc.13", SHODH_FEATURES), ("2.0.0-rc.12", SHODH_FEATURES)]))

    def test_without_download_binaries_fails(self):
        with self.assertRaisesRegex(ort.OrtError, "download-binaries"):
            ort.ort_sys_package(self.metadata([("2.0.0-rc.13", {"std"})]))

    def test_resolve_reads_the_crate_table(self):
        with tempfile.TemporaryDirectory() as tmp:
            crate = Path(tmp) / "ort-sys-2.0.0-rc.13"
            (crate / "build" / "download").mkdir(parents=True)
            (crate / "build" / "download" / "dist.tsv").write_text(DIST_TSV, encoding="utf-8")
            meta = self.metadata([("2.0.0-rc.13", SHODH_FEATURES)])
            meta["packages"][0]["manifest_path"] = str(crate / "Cargo.toml")
            calls = []

            def run(cmd):
                calls.append(cmd)
                if cmd[0] == "rustc":
                    return f"rustc 1.97.1\nhost: {WIN}\nrelease: 1.97.1\n"
                return json.dumps(meta)

            plan = ort.resolve(run)
        self.assertEqual(plan.target, WIN)
        self.assertEqual(plan.dist.sha256, "f7c654b3729cb9e5ad2a36a0c38e5b48e63bf4eed22968931aed33a0ad0b527d")
        self.assertEqual(plan.cache_key, f"onnxruntime-{WIN}-f7c654b3729cb9e5ad2a36a0c38e5b48e63bf4eed22968931aed33a0ad0b527d")
        self.assertIn("--locked", calls[1])
        self.assertEqual(calls[1][-2:], ["--filter-platform", WIN])


class ExtractTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name)

    def tearDown(self):
        self.tmp.cleanup()

    def write(self, data: bytes) -> Path:
        path = self.root / "a.tar.lzma2"
        path.write_bytes(data)
        return path

    def test_regular_files_only(self):
        big = bytes(range(256)) * 9000  # more than one read chunk once decompressed
        archive = self.write(make_archive([
            ("lib", None), ("lib/onnxruntime.lib", big), ("./include/a.h", b"x"), ("link", "symlink"),
        ]))
        dest = self.root / "out"
        self.assertEqual(ort.extract(archive, dest), 2)
        self.assertEqual((dest / "lib" / "onnxruntime.lib").read_bytes(), big)
        self.assertEqual((dest / "include" / "a.h").read_bytes(), b"x")
        self.assertFalse((dest / "link").exists())

    def test_parent_paths_are_refused(self):
        archive = self.write(make_archive([("../evil", b"x")]))
        with self.assertRaisesRegex(ort.OrtError, "outside"):
            ort.extract(archive, self.root / "out")
        self.assertFalse((self.root / "evil").exists())

    def test_absolute_paths_are_refused(self):
        archive = self.write(make_archive([("/abs", b"x")]))
        with self.assertRaisesRegex(ort.OrtError, "absolute"):
            ort.extract(archive, self.root / "out")

    def test_duplicate_entries_fail(self):
        archive = self.write(make_archive([("a", b"1"), ("a", b"2")]))
        with self.assertRaises(FileExistsError):
            ort.extract(archive, self.root / "out")

    def test_truncated_stream_fails(self):
        data = make_archive([("a", bytes(200000))])
        archive = self.write(data[: len(data) // 2])
        with self.assertRaises((ort.OrtError, tarfile.TarError, EOFError, lzma.LZMAError)):
            ort.extract(archive, self.root / "out")

    def test_empty_archive_fails(self):
        archive = self.write(make_archive([("dir", None)]))
        with self.assertRaisesRegex(ort.OrtError, "no files"):
            ort.extract(archive, self.root / "out")


def plan_for(data: bytes, root: Path) -> ort.Plan:
    sha = hashlib.sha256(data).hexdigest()
    dist = ort.Dist(target=WIN, features=frozenset({"directml"}), url="https://cdn.invalid/a.tar.lzma2", sha256=sha)
    return ort.Plan(ort_sys_version="2.0.0-rc.13", target=WIN, dist=dist, cache_dir=root / "cache")


class FetchTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name)
        self.data = make_archive([("lib/onnxruntime.lib", b"static library")])

    def tearDown(self):
        self.tmp.cleanup()

    def test_cached_archive_is_checked_and_extracted_without_download(self):
        plan = plan_for(self.data, self.root)
        archive = self.root / "archive"
        archive.write_bytes(self.data)
        ort.fetch(plan, archive, opener=self.fail_opener)
        self.assertEqual((plan.extract_dir / "lib" / "onnxruntime.lib").read_bytes(), b"static library")
        # A second run finds it extracted.
        ort.fetch(plan, archive, opener=self.fail_opener)
        ort.verify(plan)

    def test_corrupt_cache_is_replaced_by_a_download(self):
        plan = plan_for(self.data, self.root)
        archive = self.root / "archive"
        archive.write_bytes(b"corrupt")
        ort.fetch(plan, archive, opener=self.opener_returning(self.data))
        self.assertEqual(archive.read_bytes(), self.data)
        self.assertTrue(plan.extract_dir.is_dir())

    def test_download_with_wrong_hash_is_deleted(self):
        plan = plan_for(self.data, self.root)
        archive = self.root / "archive"
        with self.assertRaisesRegex(ort.OrtError, "deleted and not used"):
            ort.fetch(plan, archive, opener=self.opener_returning(b"tampered"))
        self.assertFalse(archive.exists())
        self.assertFalse(plan.extract_dir.exists())

    def test_verify_detects_a_download_by_ort_sys(self):
        plan = plan_for(self.data, self.root)
        archive = self.root / "archive"
        archive.write_bytes(self.data)
        ort.fetch(plan, archive, opener=self.fail_opener)
        (plan.extract_dir.parent / ("tmp.abc_" + H["b"])).mkdir()
        with self.assertRaisesRegex(ort.OrtError, "downloaded ONNX Runtime itself"):
            ort.verify(plan)

    def test_verify_without_fetch_fails(self):
        with self.assertRaisesRegex(ort.OrtError, "missing"):
            ort.verify(plan_for(self.data, self.root))

    @staticmethod
    def fail_opener(request, timeout):
        raise AssertionError(f"unexpected download of {request.full_url}")

    @staticmethod
    def opener_returning(data):
        def opener(request, timeout):
            return io.BytesIO(data)
        return opener


class _Handler(http.server.BaseHTTPRequestHandler):
    behaviour = "ok"
    body = b""
    hits = 0

    def do_GET(self):
        type(self).hits += 1
        if self.behaviour == "error" or (self.behaviour == "error-then-ok" and type(self).hits == 1):
            self.send_response(503)
            self.end_headers()
            return
        if self.behaviour == "missing":
            self.send_response(404)
            self.end_headers()
            return
        self.send_response(200)
        self.send_header("Content-Length", str(len(self.body) + (1000 if self.behaviour == "stall" else 0)))
        self.end_headers()
        self.wfile.write(self.body)
        self.wfile.flush()
        if self.behaviour == "stall":
            time.sleep(3)

    def log_message(self, *args):
        pass


class DownloadTest(unittest.TestCase):
    def serve(self, behaviour, body=b"payload"):
        handler = type("H", (_Handler,), {"behaviour": behaviour, "body": body, "hits": 0})
        server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        self.addCleanup(server.server_close)
        self.addCleanup(server.shutdown)
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        return handler, f"http://127.0.0.1:{server.server_port}/a", Path(self.tmp.name) / "a"

    def test_ok(self):
        _, url, dest = self.serve("ok")
        ort.download(url, dest, sleep=lambda s: None)
        self.assertEqual(dest.read_bytes(), b"payload")

    def test_retries_a_server_error(self):
        handler, url, dest = self.serve("error-then-ok")
        ort.download(url, dest, sleep=lambda s: None)
        self.assertEqual(handler.hits, 2)
        self.assertEqual(dest.read_bytes(), b"payload")

    def test_not_found_is_not_retried(self):
        handler, url, dest = self.serve("missing")
        with self.assertRaisesRegex(ort.OrtError, "HTTP 404"):
            ort.download(url, dest, sleep=lambda s: None)
        self.assertEqual(handler.hits, 1)
        self.assertFalse(dest.exists())

    def test_a_stall_fails_within_the_stall_timeout(self):
        handler, url, dest = self.serve("stall")
        started = time.monotonic()
        with self.assertRaisesRegex(ort.OrtError, "stalled"):
            ort.download(url, dest, attempts=2, stall_seconds=0.5, sleep=lambda s: None)
        self.assertLess(time.monotonic() - started, 10)
        self.assertEqual(handler.hits, 2)
        self.assertFalse(dest.exists())
        self.assertFalse(dest.with_name(dest.name + ".part").exists())

    def test_the_deadline_bounds_every_attempt(self):
        _, url, dest = self.serve("ok")
        ticks = iter(range(0, 10_000, 400))
        with self.assertRaisesRegex(ort.OrtError, "deadline"):
            ort.download(url, dest, total_seconds=600, clock=lambda: next(ticks), sleep=lambda s: None)


if __name__ == "__main__":
    unittest.main()
