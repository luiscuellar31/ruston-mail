# Contributing

Thanks for helping with Ruston Mail. Start with the
[architecture map](docs/ARCHITECTURE.md) to find the owning package, then use
the [development guide](docs/DEVELOPMENT.md) for local commands. The
[privacy guide](docs/PRIVACY.md) describes the current storage and logging
behavior.

## Make a change

1. Identify the frontend and shared code involved. Desktop state belongs in
   `src/app/`, widgets in `src/ui/`, and its SDK adapter in `src/mail/`. CLI
   commands belong in `crates/ruston-cli/`; shared Proton behavior belongs in
   `crates/ruston-core/`.
2. Make a focused change and add or adjust tests where the behavior is owned.
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
   possible; it needs no account or network connection.

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
