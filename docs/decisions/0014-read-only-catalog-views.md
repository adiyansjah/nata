# 0014: Stable read-only information_schema and pg_catalog subset

Status: accepted (by the assistant under the owner's delegation, 2026-11-07, with card 0007)
Date: 2026-11-07
Related: [ADR 0005](0005-table-format-and-catalog.md), [0007](0007-sql-dialect-and-strictness.md), [0008](0008-client-protocol.md); [card 0007](../slices/0007-wire-protocol-and-catalog-views.md)

## Context

Feature-map H makes information_schema and a pg_catalog compatibility subset T0. The current `nata-pgwire/src/session.rs` recognizes two exact ADBC type-bootstrap queries; its empty pg_attribute answer is not a schema catalog. Real schema browsers require relational joins, names, type metadata and helper functions. PostgreSQL wire compatibility must not advertise PostgreSQL storage, roles, indexes or write capabilities that Nata lacks. ADR 0005's Iceberg/catalog product decisions are not changed by exposing metadata.

## Decision (proposed; reasoned)

Use a typed, read-only metadata provider over an immutable Nata catalog snapshot, shared by information_schema, pg_catalog, SHOW/DESCRIBE, JDBC and ADBC metadata. Registered catalog → database, namespace → schema, table → relation, schema field → attribute. Each connection selects one registered catalog as its database; SHOW CATALOGS/JDBC getCatalogs enumerate registered catalogs. No automatic filesystem discovery, path/credential/options exposure, remote-user privilege simulation or mutable system tables. Search_path resolves real registered schemas; quoted identifiers follow ADR 0007. Each statement sees a consistent generation; a portal pins its bound generation until close/expiry. Refresh failure is an error, not an empty catalog.

The following is the **minimum proposed column inventory**, not a measured sufficiency claim. Standard PostgreSQL-compatible column types/nullability are part of the frozen contract, including metadata-only oid/name/regproc/regclass and OID-vector/array representations required by captured queries; these do not add general user SQL types. Preserve the existing ADBC adapter's tested type-bootstrap output until the new provider passes its default resolver: do not accidentally expose a raw function OID where that resolver expects a receive-function name. Additional columns/helpers required by actual captures must be recorded by card 0007 part 4 before implementation, with their meaning and oracle; they are not permission for general emulation.

| Relation | Minimum columns exposed |
|---|---|
| information_schema.information_schema_catalog_name | catalog_name (current database only) |
| information_schema.schemata | catalog_name, schema_name, schema_owner, default_character_set_catalog, default_character_set_schema, default_character_set_name, sql_path |
| information_schema.tables | table_catalog, table_schema, table_name, table_type, is_insertable_into |
| information_schema.columns | table_catalog, table_schema, table_name, column_name, ordinal_position, column_default, is_nullable, data_type, character_maximum_length, character_octet_length, numeric_precision, numeric_precision_radix, numeric_scale, datetime_precision, udt_catalog, udt_schema, udt_name, domain_catalog, domain_schema, domain_name, is_identity, is_generated, generation_expression, is_updatable |
| information_schema.views | table_catalog, table_schema, table_name, view_definition, check_option, is_updatable, is_insertable_into |
| information_schema.table_constraints / key_column_usage / referential_constraints | table_constraints: constraint_catalog, constraint_schema, constraint_name, table_catalog, table_schema, table_name, constraint_type, is_deferrable, initially_deferred, enforced; key_column_usage: constraint_catalog, constraint_schema, constraint_name, table_catalog, table_schema, table_name, column_name, ordinal_position, position_in_unique_constraint; referential_constraints: constraint_catalog, constraint_schema, constraint_name, unique_constraint_catalog, unique_constraint_schema, unique_constraint_name, match_option, update_rule, delete_rule. Typed empty until constraints exist |
| pg_catalog.pg_database | oid, datname, datdba, encoding, datcollate, datctype, datistemplate, datallowconn |
| pg_catalog.pg_namespace | oid, nspname, nspowner, nspacl |
| pg_catalog.pg_class | oid, relname, relnamespace, reltype, relowner, relam, reltablespace, relkind, relpersistence, relnatts, relchecks, relhasindex, relhasrules, relhastriggers, relrowsecurity, relforcerowsecurity, relispartition, reloptions, relacl, relreplident, reltuples, relpages, reltoastrelid, reloftype |
| pg_catalog.pg_attribute | attrelid, attname, atttypid, attstattarget, attlen, attnum, attndims, attcacheoff, atttypmod, attbyval, attstorage, attalign, attnotnull, atthasdef, attidentity, attgenerated, attisdropped, attislocal, attinhcount, attcollation, attoptions, attfdwoptions, attacl |
| pg_catalog.pg_type | oid, typname, typnamespace, typowner, typlen, typbyval, typtype, typcategory, typispreferred, typisdefined, typdelim, typrelid, typelem, typarray, typinput, typoutput, typreceive, typsend, typmodin, typmodout, typalign, typstorage, typnotnull, typbasetype, typtypmod, typndims, typcollation, typdefault |
| pg_catalog.pg_proc | oid, proname, pronamespace, proowner, prolang, prokind, prorettype, proretset, pronargs, proargtypes, proargnames, proargmodes, proallargtypes; only exposed callable/codec helpers, no fabricated engine functions |
| pg_catalog.pg_settings | name, setting, unit, category, short_desc, context, vartype, source, min_val, max_val, enumvals, boot_val, reset_val, pending_restart; same session registry as SHOW ALL |
| pg_catalog.pg_tables / pg_views | schemaname, tablename/viewname, tableowner/viewowner, tablespace and hasindexes/hasrules/hastriggers/rowsecurity for tables; definition for views |
| pg_catalog.pg_index / pg_indexes / pg_constraint / pg_attrdef / pg_description / pg_inherits | Typed empty when absent; standard join columns: indexrelid/indrelid/indisunique/indisprimary/indkey; schemaname/tablename/indexname/tablespace/indexdef; oid/conname/connamespace/contype/conrelid/confrelid/conkey/confkey; oid/adrelid/adnum/adbin; objoid/classoid/objsubid/description; inhrelid/inhparent/inhseqno |
| pg_catalog.pg_roles / pg_user / pg_tablespace / pg_am / pg_collation | Only compatibility identities actually referenced: oid/rolname; usesysid/usename; oid/spcname/spcowner; oid/amname/amtype; oid/collname/collnamespace/collowner/collprovider/collisdeterministic/collencoding/collcollate/collctype. No verifier, login authority, path or unsupported collation capability |

Unknown/irrelevant metadata uses standard NULL where allowed; fixed false/zero/empty is allowed only for a documented absent capability, never a guessed statistic. Read-only tables say is_insertable_into/is_updatable = NO. Defaults/keys/indexes/identity/generated expressions are absent until Nata actually implements them. Nullability follows catalog guarantees; when unknown report nullable rather than invent NOT NULL. Decimal precision/scale and type typmods must match RowDescription. TINYINT retains ADR 0008's text-wire mapping, with its Nata logical type documented separately. No supported-array claim just because clients use OID/name arrays internally in metadata; implement only frozen metadata value/format operations or fail explicitly.

Required helper surface to freeze against captures: version/current_database/current_schema/current_schemas/current_setting, format_type, pg_table_is_visible/pg_type_is_visible/pg_function_is_visible, pg_get_userbyid, pg_get_expr, pg_get_constraintdef/pg_get_indexdef/pg_get_viewdef, obj_description/col_description and encoding-name conversion. Helpers return actual definitions or NULL for absent definitions; unknown OIDs never resolve to another object. Identity/privilege helpers must describe the local trust/read-only interface, not imply authentication or RBAC. Queries outside the frozen surface return 0A000 with a safe explanation. No raw-string canned answers; bounded typed metadata operations may serve the captured relational queries independently of track A, with ordinary SQL still using nata-sql.

**OID policy:** retain PostgreSQL built-in type OIDs and their associated codec/helper identities from the pinned reference. Allocate user catalog/schema/relation/row-type OIDs in a checked noncolliding registry keyed by catalog identity plus stable table/field identity; never use a truncated unchecked hash or list order. Distinct live objects cannot collide; refresh/rename retains identity when the provider supplies it, drop/recreate gets a new identity, and exhaustion fails before publishing. Without persistent registry storage, generated OIDs are stable within a server lifetime only, not across restart; advertise and test that boundary. Do not promise an Iceberg field id survives an unrelated table replacement. Named statement revalidation on generation change must either preserve described types or error/reprepare before rows.

**Versioning:** publish a versioned Nata metadata contract tied to the tested PostgreSQL compatibility major and exact client versions. Current advertised version is only a wire-compatibility marker, not a complete PostgreSQL promise. Freeze the major from reference captures before implementation; changing it or removing/changing columns/identity semantics requires an ADR amendment and all client replays. Additive columns also run that matrix. Do not solve a failing client by claiming a newer PostgreSQL version.

## Why and alternatives considered

**Reasoned:** one provider prevents SHOW, JDBC, ADBC and psql from disagreeing. Snapshot identity and explicit OIDs prevent a tool from confusing renamed/replaced tables. A documented subset makes absence distinguishable from unsupported queries.

- **Keep two ADBC bootstrap responses:** smallest, but not schema browsing and falsely empty once registered table attributes exist.
- **Emulate all PostgreSQL catalogs:** large unrelated storage/security/type promise; violates proportion and read-only-first scope.
- **Wait for all general SQL joins/functions:** blocks independent track C. Prefer a frozen bounded metadata adapter using the existing parser; stop if capture shows it requires another general engine.
- **Expose raw connector metadata:** cannot hide credentials/options or give stable cross-client names/types; not selected.

## Evidence

- **Measured:** [4b closeout](../../bench/results/20261107-pgwire-close.md), [fixes](../../bench/results/20261107-pgwire-fixes.md) and [second fixes](../../bench/results/20261107-pgwire-fixes2.md) establish the bootstrap/default-ADBC history and scalar codec fixture only. [Debt A](../../bench/results/20261107-debt-a.md) establishes DATE output edges. No measurement certifies the inventory above.
- **Reasoned:** provider, mapping, OID/version policy and all proposed inventory/helper choices.
- **Not covered:** actual DBeaver/psql/JDBC/pgcli schema-query capture, catalog-scale cost, restart persistence, RBAC, SQL cursor snapshots, or track-B provider implementation. Card 0007 requires PostgreSQL differential fixtures, OID collision/stale-generation mutants and real-client replay before accepting sufficiency.

## Confidence, consequences and revisit when

Medium for a shared read-only provider; low for inventory sufficiency until captures pass. It adds an explicit metadata contract and bounded query surface, not writes or a PostgreSQL catalog clone. Revisit on an uncaptured query/client upgrade, new Nata type/connector, persistent identity requirement or catalog size exceeding the frozen budget. Parent must settle provider ownership and mapping of catalogs to databases before implementation.

## Amendments

- 2026-11-07, after the real-client capture against PostgreSQL (`bench/results/20261107-wire-catalog-reference.md`): the inventory is **expanded**. The default clients read 87 client/object rows; 51 were in the proposed surface and **36 were missing** (among them `pg_depend`, the policy, publication and statistics catalogs, more columns of `pg_class`, `pg_database`, `pg_index`, `pg_inherits`, `pg_proc` and `information_schema`, and 18 helper spellings). Each of the 36 is either served, read-only and derived from Nata's catalog, or refused with a stated, tested effect on that client (for example, an empty result where PostgreSQL would list policies). Silently dropping one is not allowed. The record lists every row.
- 2026-11-07, decided by the parent under the owner's delegation (track C's skeleton stopped on it): a **read-only catalog snapshot interface** owned by track C. It holds catalogs, schemas, tables, columns with their Nata types and the table's file locations, and is taken once at query or connection start and never changes during a query. In slice 10 it is implemented by a small local registry (named tables over local Parquet files, from a configuration file); slice 12 implements the same interface for an Iceberg REST catalog, and slice 11's object-store paths fit the file locations. The catalog views of this ADR read only from that snapshot.
