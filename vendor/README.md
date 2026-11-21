# Patched dependencies

`symphonia-codec-vorbis` 0.5.5 (MPL-2.0, from crates.io, by the Symphonia
project). One change, marked "AudiForm 2 patch" in `src/lib.rs`: a Vorbis
stream with more than 8 channels (multitrack Ogg, such as `.mogg` files) is
decoded with its channels in file order instead of being refused. Used
through `[patch.crates-io]` in the workspace `Cargo.toml`.
