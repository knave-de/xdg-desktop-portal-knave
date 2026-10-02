CARGO ?= cargo
PREFIX ?=
DESTDIR ?= /
.PHONY: all build install uninstall clean
all: build
build:
	$(CARGO) build --release --locked
install: build
	./scripts/install.py --no-build $(if $(PREFIX),--prefix "$(PREFIX)",--user) --destdir "$(DESTDIR)"
uninstall:
	./scripts/install.py --uninstall $(if $(PREFIX),--prefix "$(PREFIX)",--user) --destdir "$(DESTDIR)"
clean:
	$(CARGO) clean
