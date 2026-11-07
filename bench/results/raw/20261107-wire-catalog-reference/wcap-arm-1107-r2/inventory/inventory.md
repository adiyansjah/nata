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
