# Knave Portal Backend Agent Instructions

`xdg-desktop-portal-knave` is the XDG Desktop Portal backend project for the
Knave Desktop Environment. It was forked from
`xdg-desktop-portal-generic` and currently retains much of that project's
compositor-independent implementation. This repository is the place for
portal-specific Knave integration and future Knave-specific behavior.

The umbrella Knave repository owns desktop session lifecycle, user-facing
configuration, build orchestration, and cross-component compatibility policy.
Villain owns compositor behavior and Wayland protocol implementation. Keep
portal backend behavior here; coordinate any shared contract or compositor
change with its owner.

## Startup and investigation

Before editing:

1. Read this file and the umbrella Knave `AGENTS.md` for cross-repository work.
2. Read the relevant README, architecture, protocol, and packaging documents.
3. Inspect the complete owning module and search all consumers of changed
   interfaces, configuration, D-Bus names, binaries, or service files.
4. Check `git status` and preserve unrelated user changes.
5. Classify the change as private, additive, behavioral, configuration,
   migratory, protocol, or breaking.
6. For shared or externally visible changes, summarize affected components and
   compatibility before implementation.

Do not claim live portal, compositor, PipeWire, or sandbox behavior based only
on compilation or unit tests.

## Project identity and compatibility

- Treat this as the Knave fork. Do not restore upstream-only project branding,
  links, or contribution assumptions in user-facing documentation.
- Preserve the generic compositor-independent behavior where practical, and
  keep Knave-specific code clearly scoped and documented.
- Crate, executable, D-Bus, portal backend, and service identifiers still
  contain `generic`. Search all packaging and runtime consumers before
  renaming them; define a compatibility and migration path for any rename.
- Verify portal interface versions and frontend expectations against the
  supported XDG Desktop Portal behavior. Do not silently change a D-Bus
  contract or Wayland protocol requirement.
- Do not add compositor-private hooks where standard Wayland protocols suffice.
  If Villain changes are needed, identify the owning repository and both sides
  of the protocol before changing the contract.

## Runtime and resource ownership

Keep D-Bus request/session lifecycle, Wayland connections, PipeWire streams,
and worker threads explicitly owned. Review cancellation and cleanup for client
disconnect, session close, backend failure, and process shutdown. Bound queues,
buffers, retries, and per-client work. Prefer event-driven protocol handling
over polling.

Portal permissions and user consent are part of the interface contract. Do not
weaken consent, expose additional sources, or inject input through an alternate
path without documenting the security and compatibility impact.

## Code quality and verification

Follow the workspace lint and formatting configuration. Use `#[expect(...,
reason = "...")]` only where needed; do not suppress lints broadly. Keep unsafe
code, protocol sequencing, and error handling explicit.

For code changes, use the relevant checks, normally:

```console
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Use isolated D-Bus and Wayland environments for integration tests. Run live
portal tests when behavior depends on the desktop session, compositor,
PipeWire, or sandbox frontend, and report any live behavior that remains
unverified. Documentation-only changes should be checked for accuracy and
`git diff --check`.

## Git and documentation

Create a focused branch for each task, preserve unrelated changes, use
Conventional Commits, and inspect the final diff. Do not merge a branch or pull
request unless the user explicitly requests it.

Keep the README user-facing and concise. Describe inherited generic behavior
as implemented only when the current code supports it; label planned Knave
integration as planned until it exists. Update packaging, examples, and
contribution instructions when commands or runtime contracts change.
