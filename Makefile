PREFIX ?= /usr
DESTDIR ?=
LIBEXECDIR ?= $(PREFIX)/libexec
DATADIR ?= $(PREFIX)/share
# $(LIBEXECDIR)/systemd/user is not one of systemd's unit search paths
# (systemd.unit(5) lists $(PREFIX)/lib/systemd/user, not libexec) -- the
# installed unit was silently invisible to `systemctl --user`. D-Bus
# activation (the .service file installed above) worked regardless, which is
# why this went unnoticed. Ask systemd's own pkg-config file for the real
# path; fall back to the standard location if pkg-config or systemd.pc
# aren't available (e.g. cross-compiling without a target pkg-config).
SYSTEMD_USER_DIR ?= $(shell pkg-config --variable=systemduserunitdir systemd 2>/dev/null || echo $(PREFIX)/lib/systemd/user)

BINARY = xdg-desktop-portal-knave
CARGO ?= cargo

.PHONY: all build install uninstall clean

all: build

build:
	$(CARGO) build --release

install: build
	install -Dm755 target/release/$(BINARY) $(DESTDIR)$(LIBEXECDIR)/$(BINARY)
	install -Dm644 data/knave.portal $(DESTDIR)$(DATADIR)/xdg-desktop-portal/portals/knave.portal
	install -Dm644 data/org.freedesktop.impl.portal.desktop.knave.service $(DESTDIR)$(DATADIR)/dbus-1/services/org.freedesktop.impl.portal.desktop.knave.service
	install -Dm644 data/xdg-desktop-portal-knave.service $(DESTDIR)$(SYSTEMD_USER_DIR)/xdg-desktop-portal-knave.service

uninstall:
	rm -f $(DESTDIR)$(LIBEXECDIR)/$(BINARY)
	rm -f $(DESTDIR)$(DATADIR)/xdg-desktop-portal/portals/knave.portal
	rm -f $(DESTDIR)$(DATADIR)/dbus-1/services/org.freedesktop.impl.portal.desktop.knave.service
	rm -f $(DESTDIR)$(SYSTEMD_USER_DIR)/xdg-desktop-portal-knave.service

clean:
	$(CARGO) clean
