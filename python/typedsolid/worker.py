"""CAD backendを子processで実行し、停止と異常終了を呼び出し側で扱えるようにする。

OCCTのBoolean演算は入力によって終わらないことがあり、同一processで呼ぶと
呼び出し側ごと停止する。子processは新しいinterpreterで起動し、親がtimeoutで
打ち切る。forkはOCCTが内部に持つthreadの状態を複製するため使わない。

multiprocessingのspawnは使わない。spawnは子の起動時に呼び出し元の`__main__`を
読み込み直すため、`if __name__ == "__main__"`の無いscriptでは子が同じscriptを
再実行して失敗し、標準入力から実行した場合は読み込み自体に失敗する。子は
`python -m typedsolid._worker_entry`として起動し、targetはmodule名と関数名で
受け取る。親子の通信は専用のpipe 2本で行い、標準出力と標準エラーは使わない。

子で送出された例外は、pickleできる限り同じ型のまま親で再送出する。子の
tracebackはnoteとして添える。segfaultなどで結果を返さずに終了した場合は
WorkerCrashedとする。いずれも合格として扱われる経路は無い。
"""

from __future__ import annotations

from collections.abc import Callable
import importlib
from multiprocessing.connection import Connection
import os
import subprocess
import sys
import time
import traceback
from typing import Any

Progress = Callable[[str], None]

# terminateからkillへ移るまでの猶予。単位は秒。
TERMINATE_GRACE_S = 5.0


class WorkerTimeout(TimeoutError):
    """子processが制限時間内に結果を返さなかった。"""


class WorkerCrashed(RuntimeError):
    """子processが結果を返さずに終了した。"""


class WorkerError(RuntimeError):
    """子processの例外をpickleできず、型名と文面だけを持ち帰った。"""


def _resolve(module: str, qualname: str) -> Callable[..., Any]:
    target: Any = importlib.import_module(module)
    for name in qualname.split("."):
        target = getattr(target, name)
    return target


def serve(task_fd: int, result_fd: int) -> None:
    """子processの本体。taskを受け取って実行し、進行と結果をresult側へ送る。"""
    with Connection(task_fd, writable=False) as tasks:
        path, module, qualname, args = tasks.recv()
    # 親と同じ順で探索する。呼び出し元のscriptは読み込まない。
    sys.path[:] = path
    connection = Connection(result_fd, readable=False)
    started = time.monotonic()

    def progress(stage: str) -> None:
        connection.send(("progress", stage, time.monotonic() - started))

    try:
        result = _resolve(module, qualname)(progress, *args)
    except BaseException as error:
        trace = traceback.format_exc()
        try:
            connection.send(("error", error, trace))
        except Exception:
            connection.send(("error", WorkerError(f"{type(error).__name__}: {error}"), trace))
    else:
        connection.send(("result", result))
    finally:
        connection.close()


def _stop(process: subprocess.Popen[bytes]) -> None:
    if process.poll() is None:
        process.terminate()
        try:
            process.wait(TERMINATE_GRACE_S)
        except subprocess.TimeoutExpired:
            process.kill()
    process.wait()


def run(
    target: Callable[..., Any],
    *args: Any,
    timeout_s: float | None,
    progress: Progress | None = None,
) -> Any:
    """targetを子processで実行し、戻り値を返す。

    targetは新しいinterpreterから名前で引ける関数とし、第1引数に進行を報告する
    関数を受け取る。`__main__`や関数内で定義した関数は引けないため受け付けない。
    引数と戻り値はpickleできる必要がある。timeout_sがNoneなら打ち切らない。
    progressには経過秒を付けた段階名が渡る。
    """
    if timeout_s is not None and timeout_s <= 0:
        raise ValueError("timeout_s must be positive")
    module, qualname = target.__module__, target.__qualname__
    # "<locals>"と"<lambda>"は名前で引けない。
    if module == "__main__" or "<" in qualname:
        raise ValueError(f"worker target must be importable by name, got {module}.{qualname}")
    if os.name != "posix":
        raise NotImplementedError("worker isolation requires POSIX; use isolated=False")

    task_read, task_write = os.pipe()
    result_read, result_write = os.pipe()
    try:
        process = subprocess.Popen(
            [sys.executable, "-m", "typedsolid._worker_entry", str(task_read), str(result_write)],
            pass_fds=(task_read, result_write),
        )
    except BaseException:
        for descriptor in (task_read, task_write, result_read, result_write):
            os.close(descriptor)
        raise
    # 子の終了でEOFを受け取れるよう、子へ渡した端は親側で閉じる。
    os.close(task_read)
    os.close(result_write)
    receiver = Connection(result_read, writable=False)
    started = time.monotonic()
    stage = "starting worker"
    try:
        try:
            with Connection(task_write, readable=False) as tasks:
                tasks.send((list(sys.path), module, qualname, args))
        except BrokenPipeError:
            # 子がtaskを読む前に終了した。終了の理由は以下のEOFで報告する。
            pass
        while True:
            remaining = None
            if timeout_s is not None:
                remaining = timeout_s - (time.monotonic() - started)
                if remaining <= 0:
                    raise WorkerTimeout(f"backend worker exceeded {timeout_s} s during: {stage}")
            if not receiver.poll(remaining):
                continue
            try:
                message = receiver.recv()
            except EOFError:
                process.wait()
                raise WorkerCrashed(
                    f"backend worker exited with code {process.returncode} during: {stage}"
                ) from None
            if message[0] == "progress":
                stage = message[1]
                if progress is not None:
                    progress(f"[{message[2]:7.1f} s] {stage}")
            elif message[0] == "result":
                return message[1]
            else:
                error: BaseException = message[1]
                error.add_note(f"raised in backend worker:\n{message[2]}")
                raise error
    finally:
        _stop(process)
        receiver.close()
