"""NATA_PORT PG_PORT WORK: both default clients must preserve SQLSTATE 22008."""
import json
import sys
from pathlib import Path
import psycopg
import pyarrow as pa
import pyarrow.parquet as pq
from adbc_driver_postgresql import dbapi

nata_port, pg_port, work = sys.argv[1:]
work = Path(work)
work.mkdir(parents=True, exist_ok=True)
records = []
for days in [-2147483648, -2147483647, -2440589, 2145042906, 2147483647]:
    file = work / f'd{days}.parquet'
    pq.write_table(pa.table({'d': pa.array([days, None], type=pa.date32())}), file)
    nata_sql = f"SELECT d FROM '{file}'"
    pg_sql = f"SELECT ('1970-01-01'::date + ({days})) AS d"
    row = {'days': days}
    for server, port, sql in [('nata', nata_port, nata_sql), ('postgres', pg_port, pg_sql)]:
        try:
            with psycopg.connect(host='127.0.0.1', port=port, user='test', dbname='nata') as conn:
                conn.execute(sql).fetchall()
            raise AssertionError('out-of-range date accepted')
        except psycopg.DataError as error:
            assert error.sqlstate == '22008', error
            row[f'{server}_psycopg'] = error.sqlstate
        try:
            with dbapi.connect(f'postgresql://test@127.0.0.1:{port}/nata') as conn:
                with conn.cursor() as cursor:
                    cursor.execute(sql)
                    cursor.fetch_arrow_table()
            raise AssertionError('out-of-range date accepted')
        except dbapi.Error as error:
            state = getattr(error, 'sqlstate', None)
            assert state == '22008' or '22008' in str(error), error
            row[f'{server}_adbc'] = '22008'
    records.append(row)
print(json.dumps(records, indent=2))
