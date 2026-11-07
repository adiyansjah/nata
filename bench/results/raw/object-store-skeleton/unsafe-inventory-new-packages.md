# Textual `unsafe` inventory of the packages that `object_store` 0.14.2 adds to the lockfile (scripts/unsafe-inventory.py nata-objstore, 2026-11-07, Linux x86-64)

Counts keywords in the packages' own sources, features that are off included; it is an upper bound, not cargo-geiger. The lockfile gained 145 packages: 2 are ours (`nata-objstore`, `nata-objfake`), 114 are in the normal+build tree of `nata-objstore` on Linux x86-64 and are counted here, and 29 more are only built for other targets (Windows, macOS, wasm, Android) or as dev dependencies. All 114 together: 15,319 keywords, 11,285 of them in aws-lc-sys (the C TLS crypto library's bindings, a build dependency of `aws-lc-rs`).

| crate | version | `unsafe` keywords | declares |
|---|---|---|---|
| aws-lc-sys | 0.47.0 | 11285 | - |
| aws-lc-rs | 1.19.0 | 762 | - |
| ring | 0.17.14 | 456 | - |
| crc-fast | 1.10.0 | 359 | - |
| lock_api | 0.4.14 | 297 | - |
| parking_lot_core | 0.9.12 | 278 | - |
| zerovec | 0.11.9 | 236 | - |
| generic-array | 0.14.7 | 156 | - |
| httparse | 1.10.1 | 156 | - |
| zmij | 1.0.23 | 140 | - |
| quinn-udp | 0.5.16 | 138 | - |
| spin | 0.10.1 | 117 | - |
| yoke | 0.8.4 | 90 | - |
| parking_lot | 0.12.5 | 86 | - |
| chacha20 | 0.10.2 | 66 | - |
| http | 1.5.0 | 64 | - |
| hyper | 1.12.0 | 61 | - |
| hybrid-array | 0.4.15 | 37 | - |
| icu_normalizer | 2.4.0 | 35 | - |
| utf_types | 1.0.0 | 31 | - |
| serde_json | 1.0.154 | 30 | - |
| tokio-util | 0.7.20 | 29 | - |
| rustls-platform-verifier | 0.7.1 | 26 | - |
| stable_deref_trait | 1.2.1 | 25 | - |
| futures-channel | 0.3.34 | 22 | - |
| tinystr | 0.8.5 | 22 | - |
| block-buffer | 0.12.1 | 21 | - |
| icu_provider | 2.4.0 | 19 | - |
| hyper-util | 0.1.21 | 17 | - |
| zeroize | 1.9.1 | 17 | - |
| idna | 1.1.0 | 16 | - |
| rand | 0.10.3 | 16 | - |
| utf8_iter | 1.1.0 | 16 | forbid/deny |
| icu_collections | 2.4.0 | 15 | - |
| itoa | 1.0.18 | 13 | - |
| indexmap | 2.14.2 | 12 | - |
| cpufeatures | 0.3.1 | 11 | - |
| yoke-derive | 0.8.4 | 11 | - |
| itertools | 0.15.0 | 10 | - |
| potential_utf | 0.1.6 | 10 | - |
| quinn | 0.11.12 | 10 | - |
| zerovec-derive | 0.11.7 | 10 | - |
| try-lock | 0.2.5 | 8 | - |
| zerotrie | 0.2.5 | 7 | forbid/deny |
| icu_locale_core | 2.4.0 | 6 | - |
| openssl-probe | 0.2.2 | 5 | - |
| quinn-proto | 0.11.19 | 5 | - |
| reqwest | 0.13.5 | 5 | - |
| atomic-waker | 1.1.2 | 4 | - |
| percent-encoding | 2.3.2 | 4 | - |
| serde | 1.0.229 | 4 | - |
| serde_core | 1.0.229 | 4 | - |
| sync_wrapper | 1.0.2 | 4 | - |
| synstructure | 0.14.0 | 4 | - |
| scopeguard | 1.2.0 | 3 | - |
| writeable | 0.6.4 | 3 | - |
| async-trait | 0.1.92 | 2 | - |
| either | 1.19.0 | 2 | - |
| form_urlencoded | 1.2.2 | 2 | - |
| futures-macro | 0.3.34 | 2 | - |
| futures-sink | 0.3.34 | 2 | - |
| http-body | 1.1.0 | 2 | - |
| lru-slab | 0.1.3 | 2 | - |
| object_store | 0.14.2 | 2 | - |
| subtle | 2.6.1 | 2 | - |
| cmake | 0.1.58 | 1 | - |
| h2 | 0.4.20 | 1 | - |
| http-body-util | 0.1.5 | 1 | - |
| md-5 | 0.11.0 | 1 | - |
| rustls-pki-types | 1.15.1 | 1 | - |
| tower-http | 0.6.11 | 1 | forbid/deny |
| url | 2.5.8 | 1 | - |

42 further new packages contain no `unsafe` keyword: cfg_aliases, crypto-common, crypto-common, digest, digest, displaydoc, dunce, equivalent, fnv, fs_extra, futures-io, humantime, hyper-rustls, icu_normalizer_data, icu_properties, icu_properties_data, idna_adapter, ipnet, litemap, nata-objstore, quick-xml, rand_core, rand_pcg, rustc-hash, rustls, rustls-native-certs, rustls-webpki, serde_derive, serde_urlencoded, thiserror, thiserror-impl, tinyvec, tokio-rustls, tower, tower-layer, tower-service, tracing-attributes, typenum, untrusted, want, zerofrom, zerofrom-derive.
