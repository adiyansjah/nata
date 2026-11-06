"""Step 4: no driver protocol settings changed; default preparation and parameter dumps."""
import psycopg, sys, datetime, decimal, threading
port, f, large = sys.argv[1:]
print('psycopg', psycopg.__version__, 'libpq', psycopg.pq.version(), flush=True)
def conn():
    return psycopg.connect(host='127.0.0.1', port=port, user='test', dbname='nata')
with conn() as c:
    cursor = c.cursor()
    print('prepare_threshold', c.prepare_threshold)
    for _ in range(10):
        r = cursor.execute(f"SELECT * FROM '{f}'").fetchall()
        assert [type(v) for v in r[0]] == [bool,str,int,int,int,float,float,decimal.Decimal,str,datetime.date]
        assert r[0][:5] == (True,'-128',-32768,-2147483648,-9223372036854775808)
        assert str(r[0][6]) == 'nan' and str(r[0][5]) == '-0.0'
        assert r[0][7:] == (decimal.Decimal('123.450'),'',datetime.date(1970,1,1))
        assert r[1] == (False,'127',32767,2147483647,9223372036854775807,float('inf'),-float('inf'),decimal.Decimal('-0.001'),"secret-cell-a,b\né",datetime.date(2024,2,29))
        assert all(v is None for v in r[2])
    for value in [7, -32768, 2147483647, 9223372036854775807, True, 'hello', datetime.date(2024,2,29), 1.25]:
        for _ in range(10):
            row = cursor.execute(f"SELECT %s AS p FROM '{f}' LIMIT 1", (value,)).fetchone()
            assert row == (value,), (value, row)
    for _ in range(10):
        assert cursor.execute(f"SELECT CAST(%s AS INTEGER) FROM '{f}' LIMIT 1",(None,)).fetchone() == (None,)
        assert cursor.execute(f"SELECT CAST(%s AS DECIMAL(10,2)) FROM '{f}' LIMIT 1",(decimal.Decimal('1.25'),)).fetchone() == (decimal.Decimal('1.25'),)
    print('DEFAULT ten repeats, every type/value/NULL, typed parameters PASS', flush=True)
    try:
        cursor.execute(f"SELECT nope FROM '{f}'")
        raise AssertionError('missing error')
    except psycopg.Error as e:
        assert e.sqlstate == '42703' and e.diag.statement_position == '8'
        print('error', e.sqlstate, 'position', e.diag.statement_position, flush=True)
    c.rollback()
with conn() as c:
    timer=threading.Timer(.15,c.cancel); timer.start()
    try:
        c.execute(f"SELECT s FROM '{large}'").fetchall()
        raise AssertionError('not cancelled')
    except psycopg.Error as e:
        assert e.sqlstate == '57014'; print('cancel', e.sqlstate, flush=True)
    finally:
        timer.join()

with conn() as c:
    try:
        with c.cursor(name='server_cursor') as cursor:
            cursor.execute(f"SELECT si FROM '{f}'")
        raise AssertionError('SQL server cursor should be refused')
    except psycopg.Error as e:
        assert e.sqlstate == '0A000'
        print('SQL DECLARE server-side cursor', e.sqlstate)
print('PASS')
