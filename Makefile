.PHONY: check fmt test win ubuntu

LLVM_BIN := /opt/homebrew/opt/llvm/bin
WIN_TARGET := x86_64-pc-windows-msvc
UBUNTU_TARGET := x86_64-unknown-linux-gnu

check:
	cargo check

fmt:
	cargo fmt

test:
	cargo test

win:
	PATH="$(LLVM_BIN):$$PATH" cargo xwin build --release --target $(WIN_TARGET)

ubuntu:
	@command -v cargo-zigbuild >/dev/null 2>&1 || { echo "missing cargo-zigbuild: cargo install cargo-zigbuild"; exit 1; }
	@command -v zig >/dev/null 2>&1 || { echo "missing zig: install zig first"; exit 1; }
	@rustup target list --installed | grep -qx "$(UBUNTU_TARGET)" || { echo "missing rust target: rustup target add $(UBUNTU_TARGET)"; exit 1; }
	cargo zigbuild --release --target $(UBUNTU_TARGET)
