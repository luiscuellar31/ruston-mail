# Contributing

Thanks for helping with Ruston Mail. The
[development guide](docs/DEVELOPMENT.md) has local commands, and the
[privacy guide](docs/PRIVACY.md) describes local storage and logging.

## Make a change

1. Use the [architecture map](docs/ARCHITECTURE.md) to find the package that
   owns your change.
2. Make a focused change and add or adjust tests where the behavior is owned.
   Prefer readable, maintainable code consistent with the surrounding style
   over fewer lines.
   Core API requests have mock-server tests in
   `crates/ruston-core/tests/api_wiremock.rs`. Live account tests are optional
   and require explicit credentials.
3. Review `docs/ARCHITECTURE.md` for **every change**. Update it in the same
   change whenever ownership, an important flow, persistence, or a path in its
   navigation table changes. Update README or the user and privacy guides when
   their claims change. A change with no architectural impact needs no
   artificial architecture edit.
4. Run the relevant checks. In the pull request, describe the affected
   packages, observed behavior, and whether the architecture map changed or
   remains accurate. For UI changes, use the fictional desktop mailbox when
   possible; the demo itself does not contact Proton.

## Engineering rules

- Write each rule, and each value that must match another, once: Proton values
  in `ruston-core`'s `model/enums.rs`, values several views share in
  `src/ui/theme.rs`, and behavior both apps need in `ruston-core`.
- Reuse what the crate, `ruston-core`, and current dependencies provide. Add a
  trait, generic, wrapper, or crate only when a second user or a test double
  needs it.
- Use an enum or a named-field struct instead of a `bool` passed as a literal,
  several `bool`s, or a tuple of more than two items.
- Interface code reads `App` and returns `Message`s; the shell applies them in
  named steps. Sign-in fields, edited in place to keep secrets out of copies,
  are the exception.
- Keep platform code in one function per system with the same signature, not in
  `#[cfg]` branches inside a function.
- Keep slow work off the interface thread: disk, network, dialogs, the
  credential store, processes, and system calls run as `Runtime` effects, with
  blocking calls in `spawn_blocking`.
- Wake the window with `request_repaint()` when a result arrives, and never
  emit on every frame a message the app ignores.
- Comments say why, briefly; doc comments say what an item is for. Describe the
  code as it is, not how it changed; a regression test may name its failure.
- When behavior changes, update the names, docs, and tests that describe it,
  wherever they are.
- Keep a test module longer than about 300 lines in its own `tests.rs`, share
  helpers through a `test_support` module, and wait for other threads or tasks
  with a deadline or `tokio::time::timeout`, not a fixed sleep.

## Pull requests and AI assistance

AI tools are welcome. Please review the full diff yourself and make sure you
understand what you are submitting.

Use the [pull request template](.github/pull_request_template.md) to record
why the change is needed, what changed, and which checks you actually ran.

## Local checks

During development, target the affected package:

```sh
cargo test -p ruston-mail
cargo test -p ruston-cli
cargo test -p ruston-core
```

Before opening a code pull request, run the workspace checks from CI:

```sh
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --locked --workspace --no-deps --all-features
cargo test --locked --workspace --all-features
```

CI also checks dependencies and licenses with `cargo-deny`, unused
dependencies with `cargo-machete`, and advisories with `cargo-audit` in its
separate workflow. For documentation-only changes, check the links and run
`git diff --check`; code compilation is not needed to validate Markdown.

## Commits

Write commit messages in English using Conventional Commits:

```text
type(scope): imperative summary
type: imperative summary
```

Take the scope from
[`packaging/release-note-areas.tsv`](packaging/release-note-areas.tsv), which
groups scopes such as `reader`, `compose`, `cli`, `core`, or `packaging` into the
areas of the release notes. A user-visible change without a scope, or with one
not listed there, appears under Other; add an area there only for a new part of
the application. Common types here are `feat`, `fix`, `perf`, `refactor`,
`docs`, `test`, `ci`, and `chore`. Keep each commit about one logical change.
Examples:

```text
feat(cli): add offline demo mode
fix(auth): clear the local session after failed revocation
docs: map the workspace architecture
```

For a breaking change, use `!` after the type or scope and explain the impact
and migration in the commit body with a `BREAKING CHANGE:` footer.

The release notes list each subject line, grouped by type and by the area of
its scope, so choose the type by who notices the change:

- `feat`, `fix`, and `perf` are for changes that users notice: a new
  capability, corrected behavior, or faster work. That includes what they
  download and install, such as a new installer.
- Changes to CI, builds, packaging scripts, and tests use `ci`, `build`,
  `test`, or `chore`, even when they fix something.

Write the subject as the effect, not the mechanism: `fix(app): take over from
an instance that is closing` rather than `fix(app): retry the instance lock
while connecting`. The body, which the release notes do not show, can explain
how.
