SELECT n.nspname,t.typname,t.oid,t.typtypmod FROM pg_catalog.pg_type t JOIN pg_catalog.pg_namespace n ON n.oid=t.typnamespace
