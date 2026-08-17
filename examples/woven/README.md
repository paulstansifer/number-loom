# Frozen `WOVEN` share strings

Each fixture here is a group of files. `<name>.<n>.woven` are share strings exactly as some build
of number-loom wrote them, oldest first; `<name>.json` is the same document written out as
readable JSON. `golden_tests` in `src/formats/woven.rs` checks two things about every group:

1. every `.woven` in it, and the `.json`, mean the same document;
2. what the current code writes is byte-for-byte one of the `.woven` files.

After making a change to serialization, you need to acknowledge the effects by running:

    UPDATE_WOVEN_SNAPSHOTS=1 cargo test --lib golden -- --nocapture

This rewrites each `<name>.json` to match what the code now writes, and adds a `<name>.<n>.woven`
for any encoding not already on file. It never edits or removes an existing `.woven`, so it can
only add claims, not withdraw them — which is why it is safe to run. (`--nocapture` is what lets
you see the `diff` commands it prints; the old copy of each rewritten `.json` is left in `tmp/`.)

To add a fixture, add a case to `corpus()` in `src/formats/woven.rs` and run:

    cargo test --lib generate_missing_fixtures -- --ignored --nocapture

That writes `<name>.0.woven` and `<name>.json` for fixtures that have no files yet. Without
`--nocapture` you will see only "ok", including in the case where it decided to do nothing.

It will not replace a share string that already exists, so **editing a fixture's entry in
`corpus()` does not change its files** — the share strings are records of what shipped, not
outputs of `corpus()`. To rebuild a fixture from `corpus()` anyway, which is only right if it has
never been committed, delete the whole group first and then generate:

    rm examples/woven/<name>.*

A group that has lost only its `.json` is a different matter: that file is derived, so either
command above will rebuild it from the share strings that remain.
