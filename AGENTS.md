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

- Use Knave identity for this fork's package, executable, D-Bus service,
  portal backend ID, data files, documentation, and repository links. Keep
  upstream attribution and license notices where required.
- Retain generic compositor-independent behavior when it remains useful, but
  do not describe inherited capabilities as Knave-integrated unless the
  current implementation and supported session provide them.
- Treat each identity surface as a separate compatibility contract: Cargo
  package and binary names, D-Bus well-known name, service activation file,
  portal backend ID, object paths, persistent data, environment variables, and
  Rust API names can change independently.
- Preserve the documented `XDP_GENERIC_*` environment-variable fallbacks and
  the ability to read restore data written with the former `generic` vendor
  unless a migration explicitly removes them. Prefer Knave-specific settings
  when both old and new forms are present, and keep deprecation behavior
  visible to users and maintainers.
- Verify portal interface names, versions, method signatures, signals, error
  names, and frontend expectations against the supported XDG Desktop Portal
  contract. Do not make an interface appear available when its behavior is
  missing or unsupported.
- Keep persistent-data changes versioned or backward-readable. Do not silently
  reinterpret old values, drop saved selections, or change vendor identity in
  a way that strands existing sessions.
- Prefer standard Wayland protocols over compositor-private hooks. When a
  Villain extension is necessary, identify its protocol owner, negotiation
  behavior, version requirements, failure fallback, and both sides of the
  change.
- Do not add GitHub release, package publication, or other distribution
  automation unless requested. Keep version metadata internally consistent;
  the current initial project version is `0.0.1` until the project explicitly
  changes it.

## Component boundaries and state

- Keep compositor mechanisms separate from window-management and desktop
  policy. Portal code consumes compositor protocols; it does not own
  compositor policy.
- Keep D-Bus argument decoding and reply serialization separate from portal
  state mutation. Validate inputs before changing state, and make errors
  observable at the protocol boundary.
- Keep portal request state, portal session state, Wayland object state, and
  PipeWire stream state distinct. Define which object owns each transition and
  resource.
- Keep focus state separate from pointer delivery and layer-shell focus. Do
  not infer input permission or focus from pointer position or an unrelated
  protocol event.
- Avoid cross-module global state when an explicit session or request owner can
  carry the required context.
- Do not perform opportunistic refactors or unrelated cleanup. Keep each
  change focused on the requested contract or behavior.

## Protocol sequencing and runtime safety

For every affected path, review ordering and failure behavior, including:

- portal request creation, response, dismissal, cancellation, and destruction;
- session creation, restore, close, client disconnect, and backend shutdown;
- D-Bus name ownership, method dispatch, serialization, and error replies;
- Wayland registry discovery, protocol version negotiation, object lifetime,
  event ordering, and compositor disconnect;
- PipeWire negotiation, stream state, buffer ownership, frame delivery, and
  teardown;
- input modifiers and key state, focus transitions, workspace or output
  changes, surface destruction, and XWayland stacking when they affect a
  captured or controlled surface;
- frame callbacks, pacing, backpressure, and slow or disconnected consumers.

Make the owner of each fd, protocol proxy, stream, buffer, child process, task,
and session explicit. Ensure cleanup and cancellation on request completion,
portal session close, client disconnect, compositor or PipeWire failure,
reload where supported, and process shutdown. Handle partial initialization
and repeated close or cancellation safely.

Portal consent and permission checks are part of the security and behavior
contract. Do not expose additional capture sources, bypass user selection,
weaken consent, or inject input through an alternate path without an explicit
design and compatibility review. Treat application-provided titles, app IDs,
restore data, and D-Bus arguments as untrusted input; validate sizes, types,
and identifiers at the boundary.

Never claim correct capture, input, consent UI, sandbox access, or desktop
integration solely because the executable starts or unit tests pass. Report
which frontend, compositor, PipeWire implementation, and sandbox conditions
were exercised, and what remains unverified.

## Code quality and comments

- Keep error paths explicit. Propagate actionable errors instead of silently
  ignoring protocol failures or converting them into success-shaped defaults.
- Review event ordering, modifier state, workspace transitions, client and
  surface destruction, XWayland stacking, output changes, and frame pacing for
  every relevant change.
- Bound untrusted input and per-request/per-session work. Avoid hidden global
  state, duplicate subscriptions, and broad lint suppressions.
- Comments should explain invariants, protocol sequencing, safety assumptions,
  or non-obvious reasons. Do not restate obvious Rust code or write long
  comments that duplicate the implementation.
- Use narrow lint exceptions with a reason only when the exception is
  necessary. Keep unsafe code localized and document its safety conditions.
- Preserve upstream license headers, attribution, and notices when editing
  inherited code. Regenerate generated notices when the dependency set changes
  if the required generator is available; inspect and report any unavailable
  generation step rather than silently claiming the output is current.

## Build and verification

This is a Cargo workspace. For Rust implementation changes, run the relevant
workspace checks:

```console
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features
```

When checking distributable package contents, run:

```console
cargo package --workspace --all-features
```

Use `--allow-dirty` only when the working tree is intentionally uncommitted.
Run checks affected by the change; report commands not run and the reason.
Do not add tests or run unrelated expensive checks without a task reason.

For protocol or runtime changes:

- Use isolated D-Bus, Wayland, runtime, and configuration directories for
  protocol tests. Do not rely on or alter the user's active desktop session.
- Run relevant ignored integration tests for D-Bus, Wayland, layer-shell,
  XWayland, PipeWire, capture, input, or frame callbacks when those paths
  change and the required runtime is available.
- Check service activation, desktop portal descriptor, backend ID, install
  paths, and environment-variable behavior when identity or packaging changes.
- Distinguish test coverage from a live Knave/Villain session check. Report
  unverified live behavior plainly.
- After editing documentation, examples, generated files, or packaging,
  inspect the resulting contents and run `git diff --check`.

Compilation, unit tests, and a successful compositor start do not establish
acceptable resource behavior. For changes affecting long-lived runtime work,
measure CPU, resident memory, threads, file descriptors, wakeups, and relevant
latency under idle, normal, and stress workloads when the required setup is
available.

## Git, pull requests, and documentation

- Preserve unrelated dirty changes. Create a new focused branch for each
  task; do not do implementation work directly on `master`/`main`.
- Use Conventional Commits. Commit each coherent implementation slice when
  the task calls for commits, and inspect the staged diff before committing.
- Link dependent cross-repository pull requests and describe the shared
  contract, consumers, compatibility, verification, rollout order, and
  rollback path.
- Never merge, squash-merge, or otherwise integrate a branch or pull request
  unless the user explicitly requests the merge. A review approval or passing
  checks is not merge authorization. If the user requests a merge without a
  method, use squash merge.
- Do not create releases, publish packages, or enable release/publish workflows
  unless explicitly requested.
- Before completion, inspect the final diff, run `git diff --check`, check
  generated files, and report relevant behavior that remains unverified.
- Keep README and examples concise and accurate. Update them when build, run,
  configuration, protocol, packaging, or behavioral contracts change. Clearly
  distinguish implemented behavior from planned Knave integration.

## Performance and resource usage

Review resource behavior before adding portal watchers, D-Bus or Wayland
subscriptions, event-loop work, worker tasks, threads, child processes, timers,
queues, caches, PipeWire buffers, or retries.

- Keep event handling event-driven. Avoid busy loops, high-frequency polling,
  duplicate subscriptions, and retry storms.
- Bound work, queue depth, retry count, memory, and concurrency per client,
  portal session, stream, and surface. Do not create unbounded workers for
  clients, streams, outputs, or events.
- Give every activity explicit cancellation and cleanup on request completion,
  disconnect, session close, reload where supported, backend failure, and
  shutdown.
- Account for slow consumers and backpressure; do not let one client stall
  unrelated portal sessions or grow queues without limit.
- Measure CPU, resident memory, threads, file descriptors, wakeups, and
  relevant latency under idle, normal, and stress workloads for changes that
  affect long-lived resource use. Record the workload and limits; do not infer
  acceptable behavior from compilation or a short successful run.
