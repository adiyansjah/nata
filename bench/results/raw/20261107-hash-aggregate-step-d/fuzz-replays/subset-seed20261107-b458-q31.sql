-- replay: python3 -I fuzz.py replay-subset --seed 20261107 --batch 458 --query 31 --target-cmd 'python3 -I adapters/nata_adapter.py'
-- class: listed: DuckDB evaluates every operand of AND/OR in a projection; Nata only the rows the earlier operands left undecided (dialect 9b)
-- difference: oracle raised an error but the target answered: Out of Range Error: Overflow in negation of numeric value!
SET TimeZone='UTC';
CREATE TABLE t1 (id INTEGER NOT NULL, a INTEGER, b BIGINT, ti TINYINT, sm SMALLINT, c DOUBLE, r REAL, d DECIMAL(10,2), w DECIMAL(25,5), s VARCHAR, f BOOLEAN, dt DATE);
INSERT INTO t1 VALUES (7, -2, 9223372036854775807, NULL, -32768, CAST('-1.5' AS DOUBLE), -1.5, 99999999.99, -12345678901234567890.12345, NULL, false, DATE '2021-11-07'), (8, -3, -1, 5, NULL, CAST('1e308' AS DOUBLE), 0.0, -0.01, 0.00000, 'ab', false, DATE '2038-01-19');
CREATE TABLE t2 (id INTEGER NOT NULL, a INTEGER, b BIGINT, ti TINYINT, sm SMALLINT, c DOUBLE, r REAL, d DECIMAL(10,2), w DECIMAL(25,5), s VARCHAR, f BOOLEAN, dt DATE);
-- shrunk query:
SELECT (sm BETWEEN 1 AND (- sm)) AS c2 FROM t1;
