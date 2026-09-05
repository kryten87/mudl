# Convenience recipes for mudl. No logic lives here; each recipe just wraps
# the equivalent `cargo` invocation so contributors and CI run the exact same
# commands.
#
# Lint policy (documented here instead of a clippy.toml): we run clippy with
# its default lint set across the whole workspace, on all targets, denying
# every warning:
#
#     cargo clippy --workspace --all-targets -- -D warnings
#
# There are currently no crate-specific lint allow/deny overrides and no
# clippy.toml options in play (e.g. cognitive-complexity-threshold,
# type-complexity-threshold, disallowed-names/methods) that reflect a real,
# justified project choice. If/when such a choice arises it should go in a
# clippy.toml with a comment explaining why; until then an empty or
# cargo-culted clippy.toml would just be noise.

# Default recipe: same checks CI runs, in the same order.
default: ci

# Run the full CI-equivalent check sequence: formatting, lints, then tests.
ci: fmt-check lint test

# Run the test suite for every crate in the workspace.
test:
    cargo test --workspace

# Reformat the workspace in place.
fmt:
    cargo fmt

# Check formatting without modifying any files (what CI runs).
fmt-check:
    cargo fmt --check

# Run clippy across every target (lib, bins, tests, examples) in every
# workspace crate, treating warnings as errors.
lint:
    cargo clippy --workspace --all-targets -- -D warnings
