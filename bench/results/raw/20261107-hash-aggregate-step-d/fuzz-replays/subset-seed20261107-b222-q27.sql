-- replay: python3 -I fuzz.py replay-subset --seed 20261107 --batch 222 --query 27 --target-cmd 'python3 -I adapters/nata_adapter.py'
-- class: listed: DuckDB evaluates every operand of AND/OR in a projection; Nata only the rows the earlier operands left undecided (dialect 9b)
-- difference: oracle raised an error but the target answered: Conversion Error: Could not convert string '9999999999' to INT32
SET TimeZone='UTC';
CREATE TABLE t1 (id INTEGER NOT NULL, a INTEGER, b BIGINT, ti TINYINT, sm SMALLINT, c DOUBLE, r REAL, d DECIMAL(10,2), w DECIMAL(25,5), s VARCHAR, f BOOLEAN, dt DATE);
CREATE TABLE t2 (id INTEGER NOT NULL, a INTEGER, b BIGINT, ti TINYINT, sm SMALLINT, c DOUBLE, r REAL, d DECIMAL(10,2), w DECIMAL(25,5), s VARCHAR, f BOOLEAN, dt DATE);
INSERT INTO t2 VALUES (1, 3, 1, 100, -1, CAST('Infinity' AS DOUBLE), 3.4028235e38, -0.01, -1.50000, 'a', true, DATE '1970-01-01'), (2, 2147483647, -9223372036854775808, 100, 7, CAST('-Infinity' AS DOUBLE), 0.1, 0.20, 3.14159, NULL, false, DATE '2038-01-19');
-- shrunk query:
SELECT ((sm < sm) AND (2147483647 <> CAST('9999999999' AS INTEGER))) AS c1 FROM t2;
