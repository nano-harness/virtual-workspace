# Contributing

Thank you for contributing to Virtual Workspace.

## Development setup

Install a current stable Rust toolchain and Git, then run:

```bash
cargo build
cargo test
```

Before submitting a pull request, run:

```bash
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test
```

## Changes

- Keep pull requests focused and explain user-visible behavior changes.
- Add or update tests for bug fixes and new functionality, especially lifecycle and data-safety behavior.
- Update both `README.md` and `README.zh-CN.md` when commands, configuration, or behavior change.
- Test destructive operations only in temporary directories.
- Do not commit workspace configs, credentials, mountpoint contents, or build artifacts.

Report bugs and feature requests through GitHub Issues. For security reports, see `SECURITY.md`.
