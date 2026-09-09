export PATH := $(CURDIR)/.work/toolchain/bin:$(CURDIR)/.venv/bin:$(PATH)
export CARGO_HOME := $(CURDIR)/.work/cargo
export PYO3_PYTHON := $(CURDIR)/.venv/bin/python
export VIRTUAL_ENV := $(CURDIR)/.venv
export TMPDIR := $(CURDIR)/.work/tmp

.PHONY: check rust-check python-build python-test example fmt audit

check: rust-check python-test

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

example: python-build
	python examples/board_tray.py --output .work/board-tray

fmt:
	cargo fmt --all

audit:
	python scripts/audit-dependencies.py
