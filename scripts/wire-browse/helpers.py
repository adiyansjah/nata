"""Card 0007 utility comparisons, with explicit PG equivalents for Nata SHOW.
Called in compare.py's live pair; every case compares fields and raw text values.
"""
def compare_helpers(pg, nata, raw, binary_rows):
    report = []
    def check(sql, reference=None):
        r = dict(sql=sql, postgres_sql=reference or sql)
        try:
            po, pr = raw(pg, reference or sql)
            no, nr = raw(nata, sql)
            r.update(verdict='PASS' if (po, pr) == (no, nr) else 'FAIL',
                     postgres_fields=po, nata_fields=no, postgres_rows=pr, nata_rows=nr)
        except Exception as e:
            r.update(verdict='FAIL', error=str(e))
        report.append(r)
    # Match the explicitly different Nata defaults before comparisons. Version
    # markers and restricted domains are documented, never normalized silently.
    for sql in ["SET search_path=public", "SET application_name='paired'",
                "SET TimeZone='UTC'", "SET extra_float_digits=3"]:
        pg.execute(sql); nata.execute(sql)
    for key in ['application_name','client_encoding','DateStyle','TimeZone',
                'extra_float_digits','search_path','statement_timeout',
                'standard_conforming_strings','integer_datetimes',
                'default_transaction_read_only','default_transaction_isolation',
                'default_transaction_deferrable']:
        check('SHOW '+key)
        check("SELECT current_setting('"+key+"')")
        check("SELECT name,setting,unit,context,pending_restart FROM pg_catalog.pg_settings WHERE name='"+key+"' ORDER BY name")
    for sql in ["SET statement_timeout='1s'", 'SHOW statement_timeout',
                "SELECT current_setting('statement_timeout', false)",
                "SELECT set_config('statement_timeout','1500ms',false)",
                'SHOW statement_timeout', 'RESET statement_timeout', 'SHOW statement_timeout',
                "SELECT set_config('application_name','helper',false)",
                "SELECT current_setting('APPLICATION_NAME',true)", 'RESET application_name',
                'SHOW application_name', "SET DateStyle='ISO, DMY'", 'SHOW DateStyle',
                "SET DateStyle='ISO'", 'SHOW DateStyle', 'SET DateStyle=DEFAULT', 'SHOW DateStyle',
                'SET search_path=other, public', 'SHOW search_path', 'SELECT current_schema()', 'SELECT current_schemas(false)', 'SELECT current_schemas(true)',
                "SET search_path='missing'", 'SELECT current_schema()', 'SELECT current_schemas(false)',
                "SELECT current_setting('not_a_setting',true)",
                "SELECT set_config('default_transaction_read_only','OFF',false)", 'SHOW default_transaction_read_only', 'RESET ALL', 'SHOW application_name', 'SHOW statement_timeout']:
        check(sql)
    for setup in ["SET search_path=public", 'SET search_path="Quoted Schema"', "SET search_path=''"]:
        pg.execute(setup); nata.execute(setup)
        for sql in ['SELECT current_schema()', 'SELECT current_schemas(true)', 'SELECT current_schemas(false)']:
            r=dict(sql=sql, setup=setup, format='binary')
            try:
                po,pr=binary_rows(pg,sql);no,nr=binary_rows(nata,sql)
                hexrows=lambda rows:[[None if v is None else v.hex() for v in row] for row in rows]
                r.update(verdict='PASS' if (po,pr)==(no,nr) else 'FAIL',postgres_fields=po,nata_fields=no,postgres_rows_hex=hexrows(pr),nata_rows_hex=hexrows(nr))
            except Exception as e:
                r.update(verdict='FAIL',error=str(e))
            report.append(r)
    pg.execute('SET search_path=public'); nata.execute('SET search_path=public')
    # These commands are Nata syntax. Their output contracts are the projections
    # specified by card 0007 over PostgreSQL's equivalent registered fixture.
    check('SHOW CATALOGS', "SELECT catalog_name::text FROM information_schema.information_schema_catalog_name ORDER BY catalog_name")
    schemas="SELECT catalog_name::text,schema_name::text FROM information_schema.schemata WHERE schema_name IN ('empty','other','public','Quoted Schema') ORDER BY catalog_name,schema_name"
    check('SHOW SCHEMAS',schemas); check('SHOW SCHEMAS FROM nata',schemas)
    tables="SELECT table_catalog::text,table_schema::text,table_name::text,table_type::text FROM information_schema.tables WHERE table_schema='public' ORDER BY table_catalog,table_schema,table_name"
    check('SHOW TABLES',tables); check('SHOW TABLES FROM nata.public',tables)
    for name,schema,table in [('public.all_types','public','all_types'), ('nata."Quoted Schema"."Mixed Table"','Quoted Schema','Mixed Table')]:
        columns=f"SELECT column_name::text,data_type::text,is_nullable::text,ordinal_position::int4,numeric_precision::int4,numeric_scale::int4 FROM information_schema.columns WHERE table_schema='{schema}' AND table_name='{table}' ORDER BY ordinal_position"
        check('SHOW COLUMNS FROM '+name,columns); check('DESCRIBE '+name,columns)
    # SHOW ALL/pg_settings describe only the 15-entry supported registry, rather
    # than claiming PostgreSQL's hundreds of settings. Check that full registry
    # internally, retaining Nata-specific descriptions/domains/reset defaults.
    try:
        _, all_rows=raw(nata,'SHOW ALL')
        _, registry=raw(nata,'SELECT name,setting FROM pg_settings ORDER BY name')
        report.append(dict(sql='SHOW ALL registry consistency',verdict='PASS' if len(all_rows)==15 and sorted(r[:2] for r in all_rows)==sorted(registry) else 'FAIL', rows=all_rows,registry=registry))
    except Exception as e:
        report.append(dict(sql='SHOW ALL registry consistency',verdict='FAIL',error=str(e)))
    # PostgreSQL accepts some wider settings: Nata must explicitly refuse those,
    # transaction-local semantics, and metadata shapes outside this grammar.
    for sql in ['SET work_mem=1024','RESET work_mem','SHOW work_mem',
                "SELECT set_config('application_name','local',true)",
                'SET LOCAL application_name=local', 'SELECT * FROM pg_settings',
                'DESCRIBE public.missing','SHOW TABLES FROM missing',
                'SHOW SCHEMAS FROM missing']:
        try:
            raw(nata,sql)
            report.append(dict(sql=sql,verdict='FAIL',error='unexpected success'))
        except Exception as e:
            expected='42P01' if sql.startswith('DESCRIBE') else '3F000' if sql.startswith('SHOW TABLES') else '3D000' if sql.startswith('SHOW SCHEMAS') else '0A000'
            report.append(dict(sql=sql,verdict='PASS' if getattr(e,'sqlstate',None)==expected and (not expected.startswith(('42','3')) or bool(getattr(getattr(e,'diag',None),'statement_position',None))) else 'FAIL',sqlstate=getattr(e,'sqlstate',None),expected=expected,position=getattr(getattr(e,'diag',None),'statement_position',None)))
    return report
