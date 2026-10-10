# iOS-owned runtime packages

This directory is owned by `bhrumom/fabushi-ios`.

The pinned migration imports the Mahayana Rust workspace and its required local
dependencies into this directory. Normal iOS build/test/archive must use these
checked-in sources and must not clone another Fabushi repository.

The one-time source provenance is recorded in
`IOS_RUNTIME_SOURCE_IMPORT.json` after the import workflow completes.
