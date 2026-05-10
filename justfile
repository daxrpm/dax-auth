default: check

check:
    cargo check --workspace --all-targets

build:
    cargo build --workspace

run *ARGS:
    cargo run -p dax-cli -- {{ARGS}}

devices:
    cargo run -p dax-cli -- devices

fmt:
    cargo fmt --all

fmt-check:
    cargo fmt --all -- --check

lint:
    cargo clippy --workspace --all-targets -- -D warnings

test:
    cargo test --workspace

clean:
    cargo clean

ci: fmt-check lint check test
