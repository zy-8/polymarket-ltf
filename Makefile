.PHONY: check fmt test win

LLVM_BIN := /opt/homebrew/opt/llvm/bin
WIN_TARGET := x86_64-pc-windows-msvc

check:
	cargo check

fmt:
	cargo fmt

test:
	cargo test

win:
	PATH="$(LLVM_BIN):$$PATH" cargo xwin build --release --target $(WIN_TARGET)
