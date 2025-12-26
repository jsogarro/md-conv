.PHONY: build test check clean verify

# Build release binary
build:
	cargo build --release

# Run all checks (formatting, clippy, tests)
verify:
	@echo "Running cargo fmt..."
	cargo fmt --check
	@echo "Running cargo clippy..."
	cargo clippy -- -D warnings
	@echo "Running cargo test..."
	cargo test

# Run tests
test:
	cargo test

# Clean artifacts
clean:
	cargo clean
