.PHONY: help doctor init build test test-rust test-core test-python check check-rust-lint check-guidance acceptance wheel-smoke bootstrap deploy

# Cargo/PyO3 build scripts share outputs: never race embedded tests with extension builds.
.NOTPARALLEL:
UV = uv run --locked --no-sync
export PYO3_PYTHON = $(shell $(UV) python -c 'import sys; print(sys.executable)')
# A venv path can stay unchanged when its underlying Python/ABI is replaced.
export PYO3_ENVIRONMENT_SIGNATURE = $(shell $(UV) python -c 'import hashlib, sys, sysconfig; print(hashlib.sha256(repr((sys.executable, sys.base_prefix, sys.version, sysconfig.get_config_var("SOABI"))).encode()).hexdigest())')

help:
	@echo "Targets:"
	@echo "  make doctor     # Check local toolchain/system deps"
	@echo "  make init       # Sync Python deps and build extension"
	@echo "  make build      # Build extension in uv environment"
	@echo "  make test       # Run Rust and Python tests"
	@echo "  make test-core  # Check/test Rust without Python dependencies"
	@echo "  make check      # Run guidance and Rust format/lint/type checks"
	@echo "  make acceptance # Offline checks/core/Rust/fresh-native tests"
	@echo "  make check-guidance # Verify pinned guidance metadata/hashes/links"
	@echo "  make wheel-smoke WHEEL=path.whl # Isolated fresh-wheel import"
	@echo "  make bootstrap  # Full setup + checks + tests"
	@echo "  make deploy     # Sync versions and build for release"

doctor:
	@command -v uv >/dev/null || (echo "uv not found. Install: https://docs.astral.sh/uv/" && exit 1)
	@command -v cargo >/dev/null || (echo "cargo not found. Install: https://rustup.rs" && exit 1)
	@command -v pkg-config >/dev/null || (echo "pkg-config not found. Install with brew/apt." && exit 1)
	@pkg-config --exists glib-2.0 gobject-2.0 gstreamer-1.0 || \
		(echo "Missing GLib/GObject/GStreamer pkg-config entries."; \
		echo "macOS: brew install glib gstreamer pkg-config"; \
		echo "Ubuntu/Jetson: apt install libglib2.0-dev libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev"; \
		exit 1)
	@echo "doctor: OK"

init: doctor
	uv sync --locked --dev --no-install-project
	uv run --locked --no-sync pre-commit install
	uv run --locked --no-sync maturin develop --locked

build:
	uv run --locked --no-sync maturin develop --locked

test: test-rust test-python

test-rust:
	PYTHONHOME="$$(uv run --locked --no-sync python -c 'import sys; print(sys.base_prefix)')" PYTHONPATH="$$(uv run --locked --no-sync python -c 'import sysconfig; print(sysconfig.get_path("purelib"))')$${PYTHONPATH:+:$$PYTHONPATH}" uv run --locked --no-sync cargo test --locked

test-core:
	@tree="$$(cargo tree --locked --no-default-features --prefix none --format '{p}')" || exit $$?; \
	if printf '%s\n' "$$tree" | grep -E '^(pyo3(-[^ ]*)?|numpy) v'; then \
		echo "Python dependency found in core graph"; exit 1; \
	fi
	cargo check --locked --no-default-features --all-targets
	cargo clippy --locked --no-default-features --all-targets -- -D warnings
	cargo test --locked --no-default-features

test-python: build
	@if [ -d tests ]; then uv run --locked --no-sync pytest -q; else echo "No tests directory; skipping pytest."; fi

check: check-guidance
	$(UV) cargo fmt -- --check
	$(MAKE) check-rust-lint
	$(UV) cargo check --locked --tests

check-rust-lint:
	$(UV) cargo clippy --locked --all-targets --all-features -- -D warnings

check-guidance:
	$(UV) python scripts/check-guidance.py

acceptance: check test-core test-rust test-python

wheel-smoke:
	$(UV) python scripts/smoke-wheel.py "$(WHEEL)"

bootstrap: init acceptance
	@echo "bootstrap: complete"

deploy:
	./scripts/sync-version.sh "$(or $(VERSION),$(shell sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1))"
	@echo " Building Python wheel for distribution..."
	uv run --locked --no-sync maturin build --release --locked
	@echo ""
	@echo "🎉 Deploy complete!"
	@echo "   Build artifacts:"
	@echo "   - Python wheel: target/wheels/"
	@echo ""
	@echo "💡 To install: pip install target/wheels/unitree_webrtc_rs-*.whl"
