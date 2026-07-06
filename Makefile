BIN_DIR ?= $(HOME)/.local/bin
BINARY := graphify-light
RELEASE_BINARY := target/release/$(BINARY)

.PHONY: build install

build:
	cargo build --release

install: build
	mkdir -p "$(BIN_DIR)"
	install -m 755 "$(RELEASE_BINARY)" "$(BIN_DIR)/$(BINARY)"
	@echo "Installed $(BINARY) to $(BIN_DIR)/$(BINARY)"
	@echo "Make sure $(BIN_DIR) is on PATH."
