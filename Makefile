.PHONY: help doctor init build test check bootstrap

help:
	@echo "Targets:"
	@echo "  make doctor     # Check local toolchain/system deps"
	@echo "  make init       # Sync Python deps and build extension"
	@echo "  make build      # Build extension in uv environment"
	@echo "  make test       # Run Python tests (if tests/ exists)"
	@echo "  make check      # Run Rust format/lint/type checks"
	@echo "  make bootstrap  # Full setup + checks + tests"

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
	uv sync --dev
	uv run maturin develop

build:
	uv run maturin develop

test:
	@if [ -d tests ]; then uv run pytest -q; else echo "No tests directory; skipping pytest."; fi

check:
	cargo fmt -- --check
	cargo clippy --all-targets --all-features -- -D warnings
	cargo check --tests

bootstrap: init check test
	@echo "bootstrap: complete"
