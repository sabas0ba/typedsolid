"""CAD backendを子processで実行し、停止と異常終了を呼び出し側で扱えるようにする。

OCCTのBoolean演算は入力によって終わらないことがあり、同一processで呼ぶと
呼び出し側ごと停止する。子processはspawnで起動し、親がtimeoutで打ち切る。
forkはOCCTが内部に持つthreadの状態を複製するため使わない。

子で送出された例外は、pickleできる限り同じ型のまま親で再送出する。子の
tracebackはnoteとして添える。segfaultなどで結果を返さずに終了した場合は
WorkerCrashedとする。いずれも合格として扱われる経路は無い。
"""

from __future__ import annotations

from collections.abc import Callable
import multiprocessing
from multiprocessing.connection import Connection
from multiprocessing.process import BaseProcess
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


def _child(connection: Connection, target: Callable[..., Any], args: tuple[Any, ...]) -> None:
    started = time.monotonic()

    def progress(stage: str) -> None:
        connection.send(("progress", stage, time.monotonic() - started))

    try:
        result = target(progress, *args)
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


def _stop(process: BaseProcess) -> None:
    if process.is_alive():
        process.terminate()
        process.join(TERMINATE_GRACE_S)
    if process.is_alive():
        process.kill()
    process.join()


def run(
    target: Callable[..., Any],
    *args: Any,
    timeout_s: float | None,
    progress: Progress | None = None,
) -> Any:
    """targetを子processで実行し、戻り値を返す。

    targetはmodule直下の関数とし、第1引数に進行を報告する関数を受け取る。
    引数と戻り値はpickleできる必要がある。timeout_sがNoneなら打ち切らない。
    progressには経過秒を付けた段階名が渡る。
    """
    if timeout_s is not None and timeout_s <= 0:
        raise ValueError("timeout_s must be positive")
    context = multiprocessing.get_context("spawn")
    receiver, sender = context.Pipe(duplex=False)
    process = context.Process(target=_child, args=(sender, target, args), daemon=True)
    started = time.monotonic()
    process.start()
    # 子の終了でEOFを受け取れるよう、親側の送信端は閉じる。
    sender.close()
    stage = "starting worker"
    try:
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
                process.join()
                raise WorkerCrashed(
                    f"backend worker exited with code {process.exitcode} during: {stage}"
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
