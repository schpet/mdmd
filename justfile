# https://just.systems

default:
    just --list

# release a new version: `just release patch`, `just release minor`, `just release major`, or `just release 1.2.3`
release bump:
    cargo clippy -- -D warnings
    changelog release {{bump}}
    svbump write "$(changelog version latest)" package.version Cargo.toml
    cargo generate-lockfile
    jj commit -m "chore: Release mdmd version $(changelog version latest)"
    jj bookmark set main -r @-
    jj tag set "v$(changelog version latest)" -r @-
    jj git push --bookmark main
    git push origin --tags
    @echo "Released v$(changelog version latest)"

# regenerate .github/workflows/release.yml from dist-workspace.toml
dist-generate:
    dist generate
