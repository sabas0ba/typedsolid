"""worker子processの起動点。`python -m typedsolid._worker_entry <task_fd> <result_fd>`。

本体をtypedsolid.workerに置き、例外classなどを同じmoduleから参照させる。
このfileを`__main__`として実行するため、ここに定義したものは親から引けない。
"""

import sys

from typedsolid.worker import serve

if __name__ == "__main__":
    serve(int(sys.argv[1]), int(sys.argv[2]))
