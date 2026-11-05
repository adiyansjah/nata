# THROWAWAY SPIKE: how to decode Iceberg manifests (ADR 0005 rule 7, plan item 26)

Three decoders on the same files, compared for speed, memory and agreement:
* `generic`: the iceberg-plan spike's reader (apache-avro generic value tree, then typed entries) - what the planning numbers in ADR 0005 used;
* `iceberg-rust`: `iceberg::spec::Manifest::parse_avro` (iceberg-rust 0.11);
* `hand`: `src/hand.rs`, a schema-compiled Avro decoder that writes straight into compact arenas (codecs null, deflate, snappy, zstandard; bounds and counts only for the
  wanted columns if asked).

`mdec check <table>` (all three, entry by entry), `mdec check-x <dir>` (hand against iceberg-rust on manifests another engine wrote), `mdec recode` (rewrite a table with another
codec), `mdec bench <table> <arm> <rounds> [--want 1,3] [--threads N]` (one arm per process so peak memory is not mixed). Tables come from `spikes/iceberg-plan`:
`icx synth <dir> 1000000 clustered 10000`. Tests: `cargo test --release --test hand` (11 tests); `mutants/run.sh` (6 deliberate bugs, all caught). Results:
`bench/results/20261105-manifest-decode.md`. Delete the spike once ADR 0005 is accepted; the decoder, its tests and its mutants are what a product crate would start from.
