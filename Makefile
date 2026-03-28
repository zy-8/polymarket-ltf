.PHONY: check fmt test win ubuntu main-win main-ubuntu snapshot-write-ubuntu

LLVM_BIN := /opt/homebrew/opt/llvm/bin
WIN_TARGET := x86_64-pc-windows-msvc
UBUNTU_TARGET := x86_64-unknown-linux-gnu
DIST_DIR := target/dist
SNAPSHOT_WRITE_BUNDLE := snapshot-write-ubuntu-x86_64
POLYMARKET_LTF_UBUNTU_BUNDLE := polymarket-ltf-ubuntu-x86_64
POLYMARKET_LTF_WIN_BUNDLE := polymarket-ltf-windows-x86_64
UBUNTU_BUILD_CHECK = \
	command -v cargo-zigbuild >/dev/null 2>&1 || { echo "missing cargo-zigbuild: cargo install cargo-zigbuild"; exit 1; }; \
	command -v zig >/dev/null 2>&1 || { echo "missing zig: install zig first"; exit 1; }; \
	rustup target list --installed | grep -qx "$(UBUNTU_TARGET)" || { echo "missing rust target: rustup target add $(UBUNTU_TARGET)"; exit 1; }
define bundle_release
	rm -rf $(DIST_DIR)/$(1)
	mkdir -p $(DIST_DIR)/$(1)/scripts
	cp $(2) $(DIST_DIR)/$(1)/$(3)
	cp $(4) $(DIST_DIR)/$(1)/scripts/$(5)
	chmod +x $(DIST_DIR)/$(1)/$(3) $(DIST_DIR)/$(1)/scripts/$(5)
	@echo "bundle created: $(DIST_DIR)/$(1)"
endef

check:
	cargo check

fmt:
	cargo fmt

test:
	cargo test

win: main-win

ubuntu: main-ubuntu

main-win:
	PATH="$(LLVM_BIN):$$PATH" cargo xwin build --release --target $(WIN_TARGET)
	rm -rf $(DIST_DIR)/$(POLYMARKET_LTF_WIN_BUNDLE)
	mkdir -p $(DIST_DIR)/$(POLYMARKET_LTF_WIN_BUNDLE)/scripts
	cp target/$(WIN_TARGET)/release/polymarket-ltf.exe $(DIST_DIR)/$(POLYMARKET_LTF_WIN_BUNDLE)/polymarket-ltf.exe
	cp scripts/polymarket-ltf.bat $(DIST_DIR)/$(POLYMARKET_LTF_WIN_BUNDLE)/scripts/polymarket-ltf.bat
	@echo "bundle created: $(DIST_DIR)/$(POLYMARKET_LTF_WIN_BUNDLE)"

main-ubuntu:
	$(UBUNTU_BUILD_CHECK)
	cargo zigbuild --release --target $(UBUNTU_TARGET)
	$(call bundle_release,$(POLYMARKET_LTF_UBUNTU_BUNDLE),target/$(UBUNTU_TARGET)/release/polymarket-ltf,polymarket-ltf,scripts/polymarket-ltf.sh,polymarket-ltf.sh)

snapshot-write-ubuntu:
	$(UBUNTU_BUILD_CHECK)
	cargo zigbuild --release --target $(UBUNTU_TARGET) --example snapshot_write
	$(call bundle_release,$(SNAPSHOT_WRITE_BUNDLE),target/$(UBUNTU_TARGET)/release/examples/snapshot_write,snapshot-write,scripts/snapshot-write.sh,snapshot-write.sh)
