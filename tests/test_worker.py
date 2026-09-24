"""子processでの実行、打ち切り、異常終了、cacheの検証。"""

import os
from pathlib import Path
import subprocess
import sys
import tempfile
import textwrap
import time
import unittest

from typedsolid import Box, Feature, Model, Part
from typedsolid import cache as cache_module
from typedsolid.cache import Cache, clear
from typedsolid.cadquery import build, export
from typedsolid.worker import WorkerCrashed, WorkerTimeout, run


# 子processが名前で引けるよう、targetはmodule直下に置く。
def echo(progress, value):
    progress("echoing")
    return {"value": value, "pid": os.getpid()}


def sleep_for(progress, seconds):
    progress(f"sleeping pid={os.getpid()}")
    time.sleep(seconds)


def fail_with(progress, message):
    progress("failing")
    raise ValueError(message)


def exit_with(progress, code):
    progress("exiting")
    os._exit(code)


def block() -> Model:
    return Model((Part("block", (Feature("body", Box((0, 0, 0), (10, 10, 10))),)),))


class WorkerTests(unittest.TestCase):
    def test_result_and_progress_come_back_from_a_child(self):
        stages = []
        result = run(echo, [1, 2], timeout_s=60, progress=stages.append)
        self.assertEqual(result["value"], [1, 2])
        self.assertNotEqual(result["pid"], os.getpid())
        self.assertEqual(len(stages), 1)
        self.assertTrue(stages[0].endswith("echoing"), stages)

    def test_child_exception_keeps_its_type_and_traceback(self):
        with self.assertRaises(ValueError) as raised:
            run(fail_with, "broken input", timeout_s=60)
        self.assertEqual(str(raised.exception), "broken input")
        notes = "\n".join(getattr(raised.exception, "__notes__", []))
        self.assertIn("raised in backend worker", notes)
        self.assertIn("fail_with", notes)

    def test_timeout_stops_the_child_and_names_the_stage(self):
        stages: list[str] = []
        started = time.monotonic()
        with self.assertRaises(WorkerTimeout) as raised:
            run(sleep_for, 120, timeout_s=5, progress=stages.append)
        self.assertLess(time.monotonic() - started, 30)
        self.assertIn("sleeping", str(raised.exception))
        # 打ち切った子processが残っていないこと。
        pid = int(stages[-1].rsplit("pid=", 1)[1])
        with self.assertRaises(ProcessLookupError):
            os.kill(pid, 0)

    def test_exit_without_result_is_a_crash(self):
        with self.assertRaises(WorkerCrashed) as raised:
            run(exit_with, 7, timeout_s=60)
        self.assertIn("code 7", str(raised.exception))
        self.assertIn("exiting", str(raised.exception))

    def test_non_positive_timeout_is_rejected(self):
        for value in (0, -1):
            with self.subTest(value=value), self.assertRaises(ValueError):
                run(echo, None, timeout_s=value)

    def test_target_must_be_importable_by_name(self):
        def local(progress):
            return None

        for target in (local, lambda progress: None):
            with self.subTest(target=target.__qualname__), self.assertRaises(ValueError):
                run(target, timeout_s=60)


# 呼び出し元のscriptを子が読み込み直さないことを、実際のinterpreterで確かめる。
# spawnを使っていた版では、guardの無いscriptと標準入力からの実行が失敗した。
UNGUARDED_SCRIPT = textwrap.dedent("""
    import sys
    from typedsolid import Box, Feature, Model, Part
    from typedsolid.cadquery import export

    print("script body executed")
    model = Model((Part("block", (Feature("body", Box((0, 0, 0), (10, 10, 10))),)),))
    export(model, sys.argv[1])
    print("export finished")
""")


class CallerTests(unittest.TestCase):
    def setUp(self):
        Path(".work").mkdir(exist_ok=True)
        self.root = Path(tempfile.mkdtemp(dir=".work"))

    def tearDown(self):
        for path in sorted(self.root.rglob("*"), reverse=True):
            path.unlink() if path.is_file() else path.rmdir()
        self.root.rmdir()

    def assert_exported_once(self, completed, output):
        self.assertEqual(completed.returncode, 0, completed.stderr)
        self.assertEqual(completed.stdout.count("script body executed"), 1, completed.stdout)
        self.assertIn("export finished", completed.stdout)
        self.assertTrue((output / "report.json").exists())

    def test_script_without_main_guard(self):
        script = self.root / "unguarded.py"
        script.write_text(UNGUARDED_SCRIPT, encoding="utf-8")
        output = self.root / "from-script"
        completed = subprocess.run(
            [sys.executable, str(script), str(output)], capture_output=True, text=True, timeout=300,
        )
        self.assert_exported_once(completed, output)

    def test_script_from_standard_input(self):
        output = self.root / "from-stdin"
        completed = subprocess.run(
            [sys.executable, "-", str(output)], input=UNGUARDED_SCRIPT,
            capture_output=True, text=True, timeout=300,
        )
        self.assert_exported_once(completed, output)


class CacheTests(unittest.TestCase):
    def setUp(self):
        Path(".work").mkdir(exist_ok=True)
        self.root = Path(tempfile.mkdtemp(dir=".work"))

    def tearDown(self):
        for path in sorted(self.root.rglob("*"), reverse=True):
            path.unlink() if path.is_file() else path.rmdir()
        self.root.rmdir()

    def test_round_trip(self):
        store = Cache(self.root)
        key = store.key("brep", {"id": "a"})
        self.assertIsNone(store.get(key))
        store.put(key, b"shape bytes")
        self.assertEqual(store.get(key), b"shape bytes")

    def test_key_depends_on_kind_and_payload(self):
        store = Cache(self.root)
        keys = {
            store.key("brep", {"id": "a"}),
            store.key("voxel", {"id": "a"}),
            store.key("brep", {"id": "b"}),
        }
        self.assertEqual(len(keys), 3)

    def test_corrupted_entry_is_discarded(self):
        store = Cache(self.root)
        key = store.key("brep", {"id": "a"})
        store.put(key, b"shape bytes")
        path = store.directory / f"{key}.bin"
        path.write_bytes(path.read_bytes()[:-1] + b"X")
        self.assertIsNone(store.get(key))
        self.assertFalse(path.exists())

    def test_clear_removes_only_cache_entries(self):
        store = Cache(self.root)
        store.put(store.key("brep", {"id": "a"}), b"a")
        store.put(store.key("brep", {"id": "b"}), b"b")
        foreign_inside = store.directory / "notes.txt"
        foreign_inside.write_text("kept")
        foreign_outside = self.root / "other.bin"
        foreign_outside.write_bytes(b"kept")

        self.assertEqual(clear(self.root), 2)
        self.assertTrue(foreign_inside.exists())
        self.assertTrue(foreign_outside.exists())
        foreign_inside.unlink()
        self.assertEqual(clear(self.root), 0)
        self.assertFalse(store.directory.exists())

    def test_fingerprint_covers_the_backend_source(self):
        """backendのsourceを含めて作るため、実装の変更で古い結果は参照されない。"""
        fingerprint = cache_module.backend_fingerprint()
        self.assertEqual(len(fingerprint), 64)
        self.assertEqual(fingerprint, cache_module.backend_fingerprint())


class IsolatedExportTests(unittest.TestCase):
    def setUp(self):
        Path(".work").mkdir(exist_ok=True)
        self.root = Path(tempfile.mkdtemp(dir=".work"))

    def tearDown(self):
        clear(self.root)
        for path in sorted(self.root.rglob("*"), reverse=True):
            path.unlink() if path.is_file() else path.rmdir()
        self.root.rmdir()

    def test_isolated_and_in_process_exports_agree(self):
        isolated = export(block(), self.root / "isolated")
        local = export(block(), self.root / "local", isolated=False)
        self.assertEqual(isolated["report"], local["report"])
        self.assertEqual(isolated["model_sha256"], local["model_sha256"])

    def test_timeout_leaves_no_output_or_staging(self):
        # interpreterの起動とcadqueryのimportだけで1秒以上かかるため、0.2秒では必ず打ち切られる。
        with self.assertRaises(WorkerTimeout):
            export(block(), self.root / "late", timeout_s=0.2)
        self.assertEqual(list(self.root.iterdir()), [])

    def test_existing_directory_is_refused_before_starting_a_worker(self):
        existing = self.root / "existing"
        existing.mkdir()
        started = time.monotonic()
        with self.assertRaises(FileExistsError):
            export(block(), existing)
        self.assertLess(time.monotonic() - started, 1.0)

    def test_second_run_reuses_cached_parts(self):
        cache_dir = self.root / "cache"
        first: list[str] = []
        second: list[str] = []
        before = build(block(), cache_dir=cache_dir, progress=first.append)
        after = build(block(), cache_dir=cache_dir, progress=second.append)

        self.assertIn("part block: building shape", first)
        self.assertIn("part block: voxel evaluation", first)
        self.assertIn("part block: shape reused", second)
        self.assertIn("part block: voxel checks reused", second)
        self.assertEqual(before.report, after.report)
        self.assertEqual(before.shapes["block"].Volume(), after.shapes["block"].Volume())

    def test_cache_survives_an_isolated_export(self):
        cache_dir = self.root / "cache"
        stages: list[str] = []
        export(block(), self.root / "first", cache_dir=cache_dir)
        export(block(), self.root / "second", cache_dir=cache_dir, progress=stages.append)
        self.assertTrue(any(stage.endswith("part block: shape reused") for stage in stages), stages)

    def test_changed_part_is_rebuilt(self):
        cache_dir = self.root / "cache"
        build(block(), cache_dir=cache_dir)
        taller = Model((Part("block", (Feature("body", Box((0, 0, 0), (10, 10, 12))),)),))
        stages: list[str] = []
        result = build(taller, cache_dir=cache_dir, progress=stages.append)
        self.assertIn("part block: building shape", stages)
        self.assertAlmostEqual(result.shapes["block"].Volume(), 1200.0, places=6)


if __name__ == "__main__":
    unittest.main()
