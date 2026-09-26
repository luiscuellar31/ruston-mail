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

Use a scope when it clarifies the area (`cli`, `core`, `auth`, `mail`, `ui`, or
`workspace`); it is optional. Common types here are `feat`, `fix`, `perf`,
`refactor`, `docs`, `test`, `ci`, and `chore`. Keep each commit about one logical
change. Examples:

```text
feat(cli): add offline demo mode
fix(auth): clear the local session after failed revocation
docs: map the workspace architecture
```

For a breaking change, use `!` after the type or scope and explain the impact
and migration in the commit body with a `BREAKING CHANGE:` footer.
