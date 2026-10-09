export PATH := $(CURDIR)/.work/toolchain/bin:$(CURDIR)/.work/node/bin:$(CURDIR)/.venv/bin:$(PATH)
export CARGO_HOME := $(CURDIR)/.work/cargo
export PYO3_PYTHON := $(CURDIR)/.venv/bin/python
export VIRTUAL_ENV := $(CURDIR)/.venv
export TMPDIR := $(CURDIR)/.work/tmp

.PHONY: check rust-check python-build python-test js-test viewer-image viewer-check example clean-cache fmt audit

check: rust-check js-test python-test

rust-check:
	mkdir -p .work/tmp
	cargo fmt --all -- --check
	cargo clippy --workspace --all-targets --locked -- -D warnings
	cargo test -p typedsolid-core --locked

python-build:
	mkdir -p .work/tmp
	maturin develop --locked --uv

python-test: python-build
	python -m unittest discover -s tests -v

# 3D viewerのJavaScriptの検査。nodeはscripts/bootstrap-node.shか開発シェルが与える。
js-test:
	node --test tests/js/*.test.js

# 3D viewerを撮影するimage。docker/viewer/Dockerfileが版を固定する。
viewer-image:
	docker build -t typedsolid-viewer docker/viewer

# 部品間の欠陥fixtureのviewerをcontainerのChromiumで撮影し、画素を検査する。
viewer-check: python-build viewer-image
	python scripts/check-viewer.py --output .work/viewer-check

example: python-build
	python examples/board_tray.py --output .work/board-tray

clean-cache:
	python -m typedsolid.cache clear .work/cache

fmt:
	cargo fmt --all

audit:
	python scripts/audit-dependencies.py
