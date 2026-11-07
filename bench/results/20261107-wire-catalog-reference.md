# Wire/catalog PostgreSQL reference — 2026-11-07

**Verdict: reference capture complete for the scripted actions; full card-0007 freeze BLOCKED. No Nata code, build or timing.** Both clean architectures pass all 15 scripted action groups. Their 11 sequence fixtures are byte-identical and their inventories agree: **51 covered / 36 missing client-object rows**, 65 distinct client/SQL pairs, zero SQL parse failures. “Covered” means ADR 0014 names every column/helper in the row, not that Nata implements it. Generic JDBC is not DBeaver UI certification.

**Do not freeze P16 from the psql/Rust/JVM medians below.** The measured harness uses `subprocess.run(timeout=180)` with no pipe, whose child wait polls in steps up to 50 ms. This adds visible timing quantization. A local alternating control on a 20 ms sleeper gives medians 63.9995 ms with that wait, 31.6620 ms with blocking wait, and 35.0290 ms with a stderr pipe. [Samples](raw/20261107-wire-catalog-reference/local/wait-control.json). The final helper drains a stderr pipe; it has no third cloud measurement. Both original attempts failed before capture; each architecture used its one permitted rerun. No further run was launched. In-process psycopg/ADBC and browsing measurements are unaffected.

**Other freeze blockers:** distribution PostgreSQL 15.19 differs from Nata's advertised 16.0; actual DBeaver navigator/shipped-driver capture is absent; wide/NULL/decimal streaming controls, 64-session/parked-portal contention, RSS/scratch bounds, suspended cancel, and precise Parse/Bind/Execute deadline-reset semantics are unmeasured. Fixed part-3 rules therefore stop the overall freeze. This report proposes decisions; it does not amend the accepted card or ADRs.

## Method and identities

Dedicated `wirecapture` job, registered in job.sh/orchestrate.sh/vm-startup.sh and checked by test-job-chains.sh. It installs distribution PostgreSQL/psql/JDK, pinned pgjdbc 42.7.14, psycopg 3.3.8, pgcli 4.7.1, ADBC PostgreSQL 1.13.0 and pyarrow 26.0.1 (the observed item35 versions). Pglast 8.7 is an offline inventory parser, not a Nata dependency. PostgreSQL binds loopback, UTF8/trust, no TLS. Default client preference encounters SSL refusal at the proxy; no authentication/encryption claim follows.

Fixtures: `empty`, `public`, `other`, `"Quoted Schema"`; duplicate table names, quoted mixed-case identifiers, every slice-4 wire type (TINYINT represented by text), NULL and empty string, decimal precision/scale, newline/tab/quote/comma. `million` holds ordered BIGINT 0..999999. SQL setup writes are confined to the disposable synthetic reference cluster. No application writes were tested. [Setup](../../scripts/wirecapture/fixture.sql).

PostgreSQL `log_statement=all` and the transparent framed proxy capture connect/browse SQL. Frontend Parse/Query plus Bind/Execute traffic and backend frames are recorded; startup/password/BackendKeyData/cancel keys are omitted. Action-to-connection ranges distinguish clients. SQL logging is disabled and the proxy bypassed for timing. Two matching captures, [ARM raw](raw/20261107-wire-catalog-reference/wcap-arm-1107-r2/) and [x86 raw](raw/20261107-wire-catalog-reference/wcap-x86-1107-r2/), contain logs, hex frames, SQL corpus, versions, CPU description and all samples. Raw SQL/cells are synthetic only.

Measured `scripts/code-tree.sh` SHA-256: both failed attempts `415fdd8f8226c251ec9a9b2e66f7805aafbc8531d26f46f597288d8a5cdfc67d`; both completed attempts `dac955de37c840522bc6d686ca16f62507576d11f2c277411aaa5de2905ec17a`. This is a content hash, not a Git commit hash. Later changes affect the offline reducer/tests/docs and fix the subprocess wait/pin pglast in the job. The measured capture harness is retained under raw/measured-harness; do not label the final helper as cloud-measured. The common job setup installs Rust and DuckDB but builds neither Nata nor any crates/ target. The only compilation is the standalone Rust reader and Java client.

## Client query inventory

The table is the union of captured statements per client. Qualified helpers are kept as emitted. Stars expand against the actual reference columns; separate SELECT/UNION scopes avoid treating a derived-table star as a base-catalog star. The corpus is the authoritative evidence for SQL constructs, aliases, correlated subqueries and parameter spellings; this conservative reducer is not a proof of sufficient bounded metadata lowering. Two reducer controls catch missing columns/helpers, invalid SQL and derived-star/UNION regressions. Both architectures produce the same inventory.

| Client | Actions | Covered rows | Missing rows |
|---|---|---:|---:|
| psql | default connect, \dt/pattern/empty/quoted, \d, \dn, \df, \l, text and CSV \copy | 15 | 16 |
| pgjdbc | getCatalogs, getSchemas, getTables, getColumns, getPrimaryKeys, getIndexInfo, getTypeInfo, getFunctions, CopyManager export | 16 | 10 |
| psycopg | default connection; explicitly authored information_schema tables/columns and pg_type/namespace queries | 2 | 2 |
| pgcli | actual default CompletionRefresher background-refresh implementation (including separate copied connection) | 10 | 5 |
| ADBC | all five GetObjects depth spellings, GetTableSchema, bootstrap and default binary COPY fetch | 8 | 3 |

Psycopg has no automatic schema-browser API: its `SELECT *` projections intentionally reveal more columns than a narrow application needs. They are harness-selected requirements, not unsolicited driver SQL. JDBC calls use pgjdbc 42.7.14; neither DBeaver UI nor a verified DBeaver-shipped jar/init sequence was run. Pgcli completion is real, but interactive terminal rendering was not driven.

| Client | Relation or function | Columns read | Covered / missing |
|---|---|---|---|
| ADBC | function:array_agg |  | missing: object/helper |
| ADBC | function:pg_catalog.col_description |  | covered |
| ADBC | function:pg_my_temp_schema |  | missing: object/helper |
| ADBC | function:unnest |  | missing: object/helper |
| ADBC | function:version |  | covered |
| ADBC | pg_catalog.pg_attribute | attisdropped, attname, attnum, attrelid, atttypid | covered |
| ADBC | pg_catalog.pg_class | oid, relkind, relname, relnamespace | covered |
| ADBC | pg_catalog.pg_constraint | confkey, confrelid, conkey, conname, conrelid, contype | covered |
| ADBC | pg_catalog.pg_database | datname | covered |
| ADBC | pg_catalog.pg_namespace | nspname, oid | covered |
| ADBC | pg_catalog.pg_type | oid, typarray, typbasetype, typname, typreceive, typrelid, typsend, typtype | covered |
| pgcli | function:array_agg |  | missing: object/helper |
| pgcli | function:current_schemas |  | covered |
| pgcli | function:generate_subscripts |  | missing: object/helper |
| pgcli | function:pg_catalog.pg_get_expr |  | covered |
| pgcli | function:pg_get_expr |  | covered |
| pgcli | function:unnest |  | missing: object/helper |
| pgcli | pg_catalog.pg_attrdef | adbin, adnum, adrelid | covered |
| pgcli | pg_catalog.pg_attribute | atthasdef, attisdropped, attname, attnum, attrelid, atttypid | covered |
| pgcli | pg_catalog.pg_class | oid, relkind, relname, relnamespace | covered |
| pgcli | pg_catalog.pg_constraint | confrelid, conrelid, contype | covered |
| pgcli | pg_catalog.pg_database | datname | covered |
| pgcli | pg_catalog.pg_depend | deptype, objid | missing: deptype, objid |
| pgcli | pg_catalog.pg_namespace | nspname, oid | covered |
| pgcli | pg_catalog.pg_proc | oid, proallargtypes, proargdefaults, proargmodes, proargnames, proargtypes, prokind, proname, pronamespace, proretset, prorettype | missing: proargdefaults |
| pgcli | pg_catalog.pg_type | oid, typarray, typelem, typname, typnamespace, typrelid | covered |
| pgjdbc | function:array_upper |  | missing: object/helper |
| pgjdbc | function:current_catalog |  | missing: object/helper |
| pgjdbc | function:current_database |  | covered |
| pgjdbc | function:current_schemas |  | covered |
| pgjdbc | function:format_type |  | covered |
| pgjdbc | function:generate_series |  | missing: object/helper |
| pgjdbc | function:information_schema._pg_expandarray |  | missing: object/helper |
| pgjdbc | function:pg_catalog.btrim |  | missing: object/helper |
| pgjdbc | function:pg_catalog.current_schemas |  | covered |
| pgjdbc | function:pg_catalog.pg_get_expr |  | covered |
| pgjdbc | function:pg_catalog.pg_get_indexdef |  | covered |
| pgjdbc | function:pg_catalog.substring |  | missing: object/helper |
| pgjdbc | function:pg_get_function_result |  | missing: object/helper |
| pgjdbc | function:replace |  | missing: object/helper |
| pgjdbc | function:row_number |  | missing: object/helper |
| pgjdbc | pg_catalog.pg_am | amname, oid | covered |
| pgjdbc | pg_catalog.pg_attrdef | adbin, adnum, adrelid | covered |
| pgjdbc | pg_catalog.pg_attribute | attgenerated, attidentity, attisdropped, attlen, attname, attnotnull, attnum, attrelid, atttypid, atttypmod | covered |
| pgjdbc | pg_catalog.pg_class | oid, relam, relkind, relname, relnamespace, relpages, reltuples | covered |
| pgjdbc | pg_catalog.pg_constraint | conkey, conname, conrelid, contype | covered |
| pgjdbc | pg_catalog.pg_database | datallowconn, datname | covered |
| pgjdbc | pg_catalog.pg_description | classoid, description, objoid, objsubid | covered |
| pgjdbc | pg_catalog.pg_index | indexrelid, indisclustered, indisunique, indkey, indoption, indpred, indrelid | missing: indisclustered, indoption, indpred |
| pgjdbc | pg_catalog.pg_namespace | nspname, oid | covered |
| pgjdbc | pg_catalog.pg_proc | oid, prokind, proname, pronamespace, prorettype | covered |
| pgjdbc | pg_catalog.pg_type | oid, typbasetype, typinput, typname, typnamespace, typnotnull, typrelid, typtype, typtypmod | covered |
| psql | function:pg_catalog.array_to_string |  | missing: object/helper |
| psql | function:pg_catalog.array_upper |  | missing: object/helper |
| psql | function:pg_catalog.format_type |  | covered |
| psql | function:pg_catalog.generate_series |  | missing: object/helper |
| psql | function:pg_catalog.pg_encoding_to_char |  | covered |
| psql | function:pg_catalog.pg_function_is_visible |  | covered |
| psql | function:pg_catalog.pg_get_expr |  | covered |
| psql | function:pg_catalog.pg_get_function_arguments |  | missing: object/helper |
| psql | function:pg_catalog.pg_get_function_result |  | missing: object/helper |
| psql | function:pg_catalog.pg_get_statisticsobjdef_columns |  | missing: object/helper |
| psql | function:pg_catalog.pg_get_userbyid |  | covered |
| psql | function:pg_catalog.pg_relation_is_publishable |  | missing: object/helper |
| psql | function:pg_catalog.pg_table_is_visible |  | covered |
| psql | function:pg_get_expr |  | covered |
| psql | function:string_agg |  | missing: object/helper |
| psql | pg_catalog.pg_am | amname, oid | covered |
| psql | pg_catalog.pg_attrdef | adbin, adnum, adrelid | covered |
| psql | pg_catalog.pg_attribute | attcollation, attgenerated, atthasdef, attidentity, attisdropped, attname, attnotnull, attnum, attrelid, atttypid, atttypmod | covered |
| psql | pg_catalog.pg_class | oid, relam, relchecks, relforcerowsecurity, relhasindex, relhasrules, relhastriggers, relispartition, relkind, relname, relnamespace, reloftype, relowner, relpartbound, relpersistence, relreplident, relrowsecurity, reltablespace, reltoastrelid | missing: relpartbound |
| psql | pg_catalog.pg_collation | collname, oid | covered |
| psql | pg_catalog.pg_database | datacl, datcollate, datctype, datdba, daticulocale, datlocprovider, datname, encoding | missing: datacl, daticulocale, datlocprovider |
| psql | pg_catalog.pg_inherits | inhdetachpending, inhparent, inhrelid, inhseqno | missing: inhdetachpending |
| psql | pg_catalog.pg_namespace | nspname, nspowner, oid | covered |
| psql | pg_catalog.pg_policy | polcmd, polname, polpermissive, polqual, polrelid, polroles, polwithcheck | missing: polcmd, polname, polpermissive, polqual, polrelid, polroles, polwithcheck |
| psql | pg_catalog.pg_proc | oid, prokind, proname, pronamespace | covered |
| psql | pg_catalog.pg_publication | oid, puballtables, pubname | missing: oid, puballtables, pubname |
| psql | pg_catalog.pg_publication_namespace | pnnspid, pnpubid | missing: pnnspid, pnpubid |
| psql | pg_catalog.pg_publication_rel | prattrs, prpubid, prqual, prrelid | missing: prattrs, prpubid, prqual, prrelid |
| psql | pg_catalog.pg_roles | oid, rolname | covered |
| psql | pg_catalog.pg_statistic_ext | oid, stxkind, stxname, stxnamespace, stxrelid, stxstattarget | missing: oid, stxkind, stxname, stxnamespace, stxrelid, stxstattarget |
| psql | pg_catalog.pg_type | oid, typcollation | covered |
| psycopg | information_schema.columns | character_maximum_length, character_octet_length, character_set_catalog, character_set_name, character_set_schema, collation_catalog, collation_name, collation_schema, column_default, column_name, data_type, datetime_precision, domain_catalog, domain_name, domain_schema, dtd_identifier, generation_expression, identity_cycle, identity_generation, identity_increment, identity_maximum, identity_minimum, identity_start, interval_precision, interval_type, is_generated, is_identity, is_nullable, is_self_referencing, is_updatable, maximum_cardinality, numeric_precision, numeric_precision_radix, numeric_scale, ordinal_position, scope_catalog, scope_name, scope_schema, table_catalog, table_name, table_schema, udt_catalog, udt_name, udt_schema | missing: character_set_catalog, character_set_name, character_set_schema, collation_catalog, collation_name, collation_schema, dtd_identifier, identity_cycle, identity_generation, identity_increment, identity_maximum, identity_minimum, identity_start, interval_precision, interval_type, is_self_referencing, maximum_cardinality, scope_catalog, scope_name, scope_schema |
| psycopg | information_schema.tables | commit_action, is_insertable_into, is_typed, reference_generation, self_referencing_column_name, table_catalog, table_name, table_schema, table_type, user_defined_type_catalog, user_defined_type_name, user_defined_type_schema | missing: commit_action, is_typed, reference_generation, self_referencing_column_name, user_defined_type_catalog, user_defined_type_name, user_defined_type_schema |
| psycopg | pg_catalog.pg_namespace | nspname, oid | covered |
| psycopg | pg_catalog.pg_type | oid, typname, typnamespace, typtypmod | covered |

### Missing surface, deduplicated

Missing helper spellings/operations (**18**): `array_agg`, `array_to_string`, `array_upper`, `btrim`, `current_catalog`, `generate_series`, `generate_subscripts`, `information_schema._pg_expandarray`, `pg_get_function_arguments`, `pg_get_function_result`, `pg_get_statisticsobjdef_columns`, `pg_my_temp_schema`, `pg_relation_is_publishable`, `replace`, `row_number`, `string_agg`, `substring`, `unnest`. `current_catalog` is SQL-value-function syntax, not a pg_proc lookup. Aggregate/window/array operations are required bounded query operations, not permission to add general user SQL.

| Relation | Missing columns |
|---|---|
| information_schema.columns | character_set_catalog, character_set_name, character_set_schema, collation_catalog, collation_name, collation_schema, dtd_identifier, identity_cycle, identity_generation, identity_increment, identity_maximum, identity_minimum, identity_start, interval_precision, interval_type, is_self_referencing, maximum_cardinality, scope_catalog, scope_name, scope_schema |
| information_schema.tables | commit_action, is_typed, reference_generation, self_referencing_column_name, user_defined_type_catalog, user_defined_type_name, user_defined_type_schema |
| pg_catalog.pg_class | relpartbound |
| pg_catalog.pg_database | datacl, daticulocale, datlocprovider |
| pg_catalog.pg_depend | deptype, objid |
| pg_catalog.pg_index | indisclustered, indoption, indpred |
| pg_catalog.pg_inherits | inhdetachpending |
| pg_catalog.pg_policy | polcmd, polname, polpermissive, polqual, polrelid, polroles, polwithcheck |
| pg_catalog.pg_proc | proargdefaults |
| pg_catalog.pg_publication | oid, puballtables, pubname |
| pg_catalog.pg_publication_namespace | pnnspid, pnpubid |
| pg_catalog.pg_publication_rel | prattrs, prpubid, prqual, prrelid |
| pg_catalog.pg_statistic_ext | oid, stxkind, stxname, stxnamespace, stxrelid, stxstattarget |

Additional grammar needed by the recorded corpus includes joins, correlated subqueries, UNION, CASE/COALESCE/NULLIF, regex/LIKE, ANY, casts to regclass/regproc/OID vectors, array subscripts and composite-returning `_pg_expandarray`. Missing relations may be truthful typed empties for absent Nata capabilities, but binding and helper semantics cannot be skipped because a fixture has no matching row. Parent scope resolution is needed if this cannot fit bounded metadata operations independent of track A.

## Replayable backend sequences

[ARM fixtures](../../fixtures/wirecapture/postgresql-15.19-arm/) and [x86 fixtures](../../fixtures/wirecapture/postgresql-15.19-x86/) each contain 11 JSON fixtures, all byte-identical across CPUs. Each records ordered frontend messages/groups and backend tag/payload hex, including explicit Flush/Sync. Only live-session CancelRequest keys are regenerated. Error implementation source fields are tied to PG 15.19; catalog OIDs require the declared identity mapping on another server. Run fixture.sql then `python3 scripts/wirecapture/replay.py FIXTURE.json` against matching PostgreSQL on loopback 55432. Local PG 17.11 generated and byte-replayed all 11 local controls successfully; PG 15.19 fixture files were captured on both cloud CPUs, not subsequently replayed after their VMs were deleted.

| Fixture | Observed PostgreSQL behavior |
|---|---|
| execute-limit-0/1/2/3/4 | BEGIN; named Parse/Bind/Describe; repeated Execute(maxRows=2)+Flush; Sync; Close portal+Flush; Sync; ROLLBACK. ParseComplete/BindComplete precede RowDescription. No duplicate RowDescription on Execute. |
| exact two/four rows | Full-sized final fetch still emits `DataRow × 2, PortalSuspended`. Next Execute emits `CommandComplete SELECT 0`; it does not announce completion on that exact boundary. |
| three rows | First Execute gives two rows+suspension; second gives one row+`SELECT 1`. Completion counts the current Execute, not the portal's cumulative total. Another Execute on a completed portal gives `SELECT 0`. |
| zero/one row | Empty gives `SELECT 0`; one gives one row+`SELECT 1` without suspension. |
| sql-cursor | BEGIN/DECLARE CURSOR; FETCH 2, FETCH 2, FETCH 0, CLOSE CURSOR, COMMIT; every fetch has RowDescription, fetched rows, FETCH n, ReadyForQuery T; COMMIT returns I. SQL cursor support remains outside Nata's card boundary. |
| timeout | SET statement_timeout=20; pg_sleep => 57014 + ReadyForQuery I; next SELECT succeeds. This tests integer milliseconds only. |
| extended-timeout-sync | ParseComplete, BindComplete, ErrorResponse 57014; an intervening Parse is ignored; Sync restores ReadyForQuery I; SELECT succeeds. |
| rollback | BEGIN => T; division by zero 22012 => E; next query 25P02 => E; ROLLBACK => I; SELECT succeeds. |
| concurrent-cancel | Separate CancelRequest while pg_sleep executes => 57014, ReadyForQuery I; same connection executes SELECT successfully. No key bytes stored. |
| copy | Query-form binary and text STDOUT; table-form CSV with HEADER. CopyOutResponse, CopyData payloads, backend CopyDone, `COPY 3`, ReadyForQuery. Binary numeric/date/NULL and text/CSV escaping are preserved byte-for-byte. Actual ADBC's query wrapper and psql/JDBC spellings are additionally in SQL corpus. |

**Proposed ADR 0015 correction:** replace “PortalSuspended when more execution remains” with the observed Execute-bound rule, including suspension at an exact multiple and per-Execute completion count. No protocol promise about timeout resetting on a new Execute follows from the single extended timeout fixture. Suspended-idle cancel delivery, delayed Parse/Bind/pipelined timeout reset points, Execute(0) on a row-producing portal, and COPY with a positive Execute limit remain uncaptured in this reference; do not guess their contract from these traces.

## Reference timings

Warm-cache million BIGINT sequential table scan (`SELECT v FROM million`), matching slice-4b's shape/order and text/raw/copy clients; this new JDBC arm uses fetchSize=1000 with autocommit off. Every value is checked before timing; Rust/JDBC/Python also validate while consuming. Psql timing discards formatted output after a separate ordered-value check. Raw text and binary are standalone std-only clients. Python ADBC includes Arrow materialization and to_pylist validation, unlike a discard sink. Connection/bootstrap is included in all current arms; interpreter startup is excluded for Python, included for psql/Rust/JVM processes. These are explicit new matching contracts, not interchangeable with slice-4b's persistent raw reader/startup-excluded timing. No cross-client or cross-format ratio is meaningful.

Five primary/twin rounds alternate order; each arm and the catalog sweeps settled below load 0.5. All **120 streaming samples and 30 browse samples per CPU** are retained. PostgreSQL per-gather worker ceilings 0/7 are labels “1/8”; the scan may execute serially. They do not demonstrate Nata-style worker shares or eight-way CPU scaling. Engine/source time and first-row latency were not isolated. No pure codec-cost claim follows.

Psycopg/ADBC rows below are valid reference timings for these exact sinks. **Psql/raw/JDBC rows have the polling defect and are diagnostic only**, despite low median twin differences.

| CPU | configured workers | Client | Primary min / median / max seconds | Twin median seconds | Status |
|---|---:|---|---:|---:|---|
| arm | 1 | adbc-copy | 0.196077 / 0.196251 / 0.202994 | 0.197068 | reference |
| arm | 1 | jdbc-fetch1000 | 0.364693 / 0.364762 / 0.364888 | 0.364763 | polling-affected; no freeze |
| arm | 1 | psql | 0.164280 / 0.164437 / 0.214521 | 0.164332 | polling-affected; no freeze |
| arm | 1 | psycopg | 0.478510 / 0.479645 / 0.480694 | 0.480101 | reference |
| arm | 1 | raw-binary | 0.113837 / 0.113995 / 0.114190 | 0.114097 | polling-affected; no freeze |
| arm | 1 | raw-text | 0.114058 / 0.114251 / 0.114323 | 0.114111 | polling-affected; no freeze |
| arm | 8 | adbc-copy | 0.196381 / 0.197503 / 0.197863 | 0.197320 | reference |
| arm | 8 | jdbc-fetch1000 | 0.314805 / 0.364804 / 0.364853 | 0.364806 | polling-affected; no freeze |
| arm | 8 | psql | 0.164026 / 0.164068 / 0.164498 | 0.164232 | polling-affected; no freeze |
| arm | 8 | psycopg | 0.478283 / 0.479440 / 0.480254 | 0.478729 | reference |
| arm | 8 | raw-binary | 0.113884 / 0.113986 / 0.114216 | 0.114140 | polling-affected; no freeze |
| arm | 8 | raw-text | 0.113919 / 0.114003 / 0.114361 | 0.114009 | polling-affected; no freeze |
| x86 | 1 | adbc-copy | 0.245350 / 0.245785 / 0.248900 | 0.244873 | reference |
| x86 | 1 | jdbc-fetch1000 | 0.414577 / 0.414731 / 0.464739 | 0.414758 | polling-affected; no freeze |
| x86 | 1 | psql | 0.214257 / 0.214286 / 0.214392 | 0.214242 | polling-affected; no freeze |
| x86 | 1 | psycopg | 0.456411 / 0.465728 / 0.467703 | 0.468172 | reference |
| x86 | 1 | raw-binary | 0.163946 / 0.164012 / 0.164044 | 0.164031 | polling-affected; no freeze |
| x86 | 1 | raw-text | 0.163988 / 0.164002 / 0.164070 | 0.163989 | polling-affected; no freeze |
| x86 | 8 | adbc-copy | 0.237661 / 0.239885 / 0.241771 | 0.240548 | reference |
| x86 | 8 | jdbc-fetch1000 | 0.414645 / 0.414856 / 0.464835 | 0.414695 | polling-affected; no freeze |
| x86 | 8 | psql | 0.214174 / 0.214288 / 0.214320 | 0.214225 | polling-affected; no freeze |
| x86 | 8 | psycopg | 0.468947 / 0.476569 / 0.481255 | 0.475180 | reference |
| x86 | 8 | raw-binary | 0.163987 / 0.164012 / 0.164475 | 0.163994 | polling-affected; no freeze |
| x86 | 8 | raw-text | 0.163990 / 0.164010 / 0.164136 | 0.164143 | polling-affected; no freeze |

Browsing uses a persistent default psycopg connection and two explicit information_schema queries (tables, then columns), fully decoded into Python tuples. Every sixteenth table is 64 columns wide; others have one column. This measures that projection, not full JDBC/ADBC/psql navigator refresh at scale. The small mixed-type fixtures were used for browsing/copy semantics, not wide streaming timing.

| CPU | Tables | Columns | JSON representation bytes | Primary median ms | Twin median ms | Proposed median ceiling ms (1.20× primary) |
|---|---:|---:|---:|---:|---:|---:|
| arm | 64 | 316 | 11423 | 2.689 | 2.765 | 3.227 |
| arm | 1024 | 5056 | 188465 | 13.612 | 13.789 | 16.334 |
| arm | 10000 | 49375 | 1889417 | 119.368 | 118.438 | 143.241 |
| x86 | 64 | 316 | 11423 | 3.149 | 3.225 | 3.779 |
| x86 | 1024 | 5056 | 188465 | 16.355 | 15.898 | 19.625 |
| x86 | 10000 | 49375 | 1889417 | 150.911 | 150.159 | 181.093 |

## Proposed thresholds and parent decisions

These are proposals for the parent, never silently frozen amendments. No threshold is relaxed to hide a miss.

| Card part-2 row | Proposed frozen rule / disposition | Evidence and reason |
|---|---|---|
| Correctness | Keep exact types/NULLs/counts/state, zero unexplained mismatches | Freeze the captured PG15.19 sequences/corpus as a versioned oracle; settle PG16 version/projections first. Exact boundary semantics need the ADR 0015 correction above. |
| Performance | Retain candidate streaming ≤1.10× PostgreSQL separately per CPU/client/format; **overall freeze blocked** | Valid Python twin median ratios are at most 1.0053; a 10% gate exceeds that observed median noise. This does not prove Nata attainability. Subprocess arms need one newly authorized corrected reference run. Small-query/prepare/reuse ≤1.20× pre-change Nata stays provisional: no pre-change Nata was built/timed here. Wide/NULL/decimal controls and startup/sink alignment remain required. |
| Scalability | Retain 64 sessions and table scales 64/1024/10000; propose median browse ≤1.20× matched PG projection, separately per CPU/scale | Explicit ceilings above; observed primary/twin medians differ under 3%. Only the two information_schema projections are measured. Full-client browsing, 64-session progress and 1/8 admission-contention budgets remain unfrozen. |
| Reliability | Keep candidate ≤2 seconds cancel/timeout/expiry cleanup; do not claim empirical freeze | Captured SQLSTATE/recovery is exact, but no firing-to-cleanup latency, load/parked-portal test or lifecycle model was measured. |
| Fault tolerance | Keep explicit errors, no false success trailers, Sync recovery and healthy-session isolation | Timeout/error/cancel fixtures cover finite legal sequences, not malformed framing/source faults or partial-output races. Inherited limits unchanged. |
| Resource bounds | Keep existing frame/bridge/object contracts; leave new RSS and 16 MiB metadata scratch/cache adequacy **unfrozen** | 10k-table result JSON is 1,889,417 bytes. That is a payload witness, not a snapshot/AST/cache/RSS bound. No allocator/cgroup/FD sampling or fresh descriptor-limit capture was run. |
| Idle portal expiry (resource/reliability) | Retain 5 seconds as a reasoned candidate only | PostgreSQL capture does not select an optimum Nata lease; loaded starvation/lifecycle evidence is required before freezing. No transparent resume promise. |
| Portability | Same semantic/performance/resource rules on both CPUs | Inventory and all 11 byte fixtures match. Python timings exist on both; process timing and resource gates remain open. |
| Compatibility | Keep every mandatory action passing or owner-approved explicit deviation | All scripted PostgreSQL actions pass; actual DBeaver UI/shipped version/init and full Nata replays absent. Do not mark this mandatory cell PASS. |
| Security/privacy | Retain loopback trust/refusal, no TLS/auth claim, bounds/leak rules | Synthetic captures only; BackendKeyData/startup/passwords/cancel secrets omitted. No security-system expansion. |
| Operability | Keep safe SQLSTATE/position/hint and lifecycle counters contract | Error fixtures supply SQLSTATE/state bytes. Nata counters, EXPLAIN metadata source and source-error redaction were not tested here. |

**Proposed ADR 0014 amendment:** add/version the missing inventory above, explicitly including current_catalog, metadata-only aggregate/window/array/composite operations and the psql policy/publication/statistics probes; decide whether the harness-selected psycopg SELECT * surface is mandatory or replace it with a separately frozen explicit projection. Empty indexes/policies/publications/statistics are acceptable only as truthful typed relations with correctly bindable helpers. Pin a compatibility major after resolving the PG15/16 mismatch. Do not claim the existing minimum inventory sufficient.

**Proposed card amendment:** link this capture, mark DBeaver UI and the missing cases explicitly blocked, freeze only the measured semantic subcorpus after version resolution, and authorize a corrected PostgreSQL-only process-timing run if desired. Align connection lifetime, validation and sinks with future Nata arms before P16. No Nata performance work should precede that resolution. No card or ADR was edited by this task.

## Cloud runs, spend and cleanup

All plans use Spot us-central1, 8 vCPU/16 GiB, 20 GiB boot plus 30 GiB data, ephemeral external IPv4, no service account/scopes. Full 2.75-hour bounds allow fifteen ten-minute settle waits plus setup/sweep margin. Cached prices.env rates (November 3/4) are estimates, not a fresh billing-catalog fetch or invoice; the orchestrator's worst case, not the shorter expected duration, was used for approval. Each bound is below the user's $0.35/run cap. No fallback machine, preemption/retry beyond the single allowed rerun, bucket or service-account change occurred. Cleanup only touched each run's labels.

| Run ID | Machine | Counted VM seconds | VM compute estimate USD | Plan worst USD | Outcome | Post-run proof |
|---|---|---:|---:|---:|---|---|
| wcap-arm-1107 | n4a-highcpu-8 | 185 | 0.005 | .324 | failed before capture: rustc cwd/toolchain | [empty instances/disks](raw/20261107-wire-catalog-reference/wcap-arm-1107/status.txt) |
| wcap-arm-1107-r2 | n4a-highcpu-8 | 336 | 0.009 | .324 | complete capture; process timing defective | [empty instances/disks](raw/20261107-wire-catalog-reference/wcap-arm-1107-r2/status.txt) |
| wcap-x86-1107 | c3d-highcpu-8 | 193 | 0.004 | .217 | failed before capture: rustc cwd/toolchain | [empty instances/disks](raw/20261107-wire-catalog-reference/wcap-x86-1107/status.txt) |
| wcap-x86-1107-r2 | c3d-highcpu-8 | 334 | 0.006 | .217 | complete capture; process timing defective | [empty instances/disks](raw/20261107-wire-catalog-reference/wcap-x86-1107-r2/status.txt) |

Sum of rounded VM estimates **$0.024**. Disk/IP charges are not invoices here; worst-case approval includes disk/IP plus the ARM Hyperdisk allowance. All four reports say resources left after cleanup: none; explicit `orchestrate.sh status --run-id` outputs independently show empty instance/disk lists. No cleanup is outstanding. Working trees were shipped, not assumed equal to HEAD.

## Local verification and limits

TMPDIR stayed on disk under the home directory. One-at-a-time checks: Python syntax compilation; shell syntax; framing fragmentation and malformed-length control (2 PASS); standalone optimized Rust compile and one-million-row binary-value validation; full job-chain suite PASS; inventory reducer canary PASS; local PG17 sequence generation and all 11 byte replays PASS; local psql/psycopg/pgcli/ADBC action smoke PASS. Java/javac was absent locally, recorded rather than skipped as a pass; both cloud Java compiles/metadata calls passed. The exact JVM/JDK build was not retained, only pgjdbc 42.7.14 and the distribution package selection; that reproducibility field remains open. The default local Python lacked ensurepip, so the offline reducer used an isolated uv/Python3.12 environment. These local smoke checks are not clean-VM timing evidence. No Cargo build, Rust workspace checks or Nata test execution was needed for these script/fixture-only changes.

The final process-wait fix was demonstrated by the local alternating sleeper control, not remeasured on GCP. Deferred evidence is explicitly open, so part 4 does not authorize Nata implementation/timing or imply an accepted freeze. No push or merge.

Final committed-tool content hash (`scripts/code-tree.sh`, before commit): `877e60f511cdf786b9f7ff4dc0978a53dbc5aa8433327eaf6bcdc2c84dc7f045`. This differs from the measured hashes for the disclosed offline reducer/tests/docs and unmeasured wait fix.


## Corrected streaming reference (run `wcap-*-1107-r3`, 2026-11-07, added by the parent)

The two permitted attempts above measured psql, the Rust raw-protocol client and pgjdbc through a harness whose child wait polled in steps up to
50 ms. The harness was fixed (`scripts/wirecapture/capture.py`, `run()`), and **one new run with the fixed harness** was made on both CPU types.
Same fixtures, settled, 5 alternating rounds with a twin. Compute $0.009 (ARM64) and $0.007 (x86-64); `orchestrate.sh status` showed nothing
left after each run. Raw: `raw/20261107-wire-catalog-r3/`. PostgreSQL 15.19, 1M BIGINT rows, medians in seconds:

| CPU | workers | client | primary | twin |
|---|---:|---|---:|---:|
| arm (n4a-highcpu-8) | 1 | adbc-copy | 0.2011 | 0.2012 |
| arm (n4a-highcpu-8) | 1 | jdbc-fetch1000 | 0.3225 | 0.3208 |
| arm (n4a-highcpu-8) | 1 | psql | 0.1672 | 0.1674 |
| arm (n4a-highcpu-8) | 1 | psycopg | 0.4796 | 0.4793 |
| arm (n4a-highcpu-8) | 1 | raw-binary | 0.1081 | 0.1083 |
| arm (n4a-highcpu-8) | 1 | raw-text | 0.1036 | 0.1030 |
| arm (n4a-highcpu-8) | 8 | adbc-copy | 0.2016 | 0.1989 |
| arm (n4a-highcpu-8) | 8 | jdbc-fetch1000 | 0.3265 | 0.3235 |
| arm (n4a-highcpu-8) | 8 | psql | 0.1611 | 0.1608 |
| arm (n4a-highcpu-8) | 8 | psycopg | 0.4756 | 0.4761 |
| arm (n4a-highcpu-8) | 8 | raw-binary | 0.0995 | 0.0991 |
| arm (n4a-highcpu-8) | 8 | raw-text | 0.0950 | 0.0948 |
| x86 (c3d-highcpu-8) | 1 | adbc-copy | 0.2417 | 0.2407 |
| x86 (c3d-highcpu-8) | 1 | jdbc-fetch1000 | 0.4057 | 0.4221 |
| x86 (c3d-highcpu-8) | 1 | psql | 0.1833 | 0.1831 |
| x86 (c3d-highcpu-8) | 1 | psycopg | 0.4503 | 0.4501 |
| x86 (c3d-highcpu-8) | 1 | raw-binary | 0.1195 | 0.1200 |
| x86 (c3d-highcpu-8) | 1 | raw-text | 0.1202 | 0.1206 |
| x86 (c3d-highcpu-8) | 8 | adbc-copy | 0.2407 | 0.2398 |
| x86 (c3d-highcpu-8) | 8 | jdbc-fetch1000 | 0.3934 | 0.3888 |
| x86 (c3d-highcpu-8) | 8 | psql | 0.1762 | 0.1768 |
| x86 (c3d-highcpu-8) | 8 | psycopg | 0.4509 | 0.4484 |
| x86 (c3d-highcpu-8) | 8 | raw-binary | 0.1123 | 0.1127 |
| x86 (c3d-highcpu-8) | 8 | raw-text | 0.1134 | 0.1133 |

Every twin is within about 1-4% of its primary, so these cells are valid references. Card 0007's streaming target (≤1.10x PostgreSQL with the
same client) is now frozen for all six clients on both CPU types.
