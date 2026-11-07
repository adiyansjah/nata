-- replay: python3 -I fuzz.py replay-subset --seed 20261107 --batch 216 --query 24 --target-cmd 'python3 -I adapters/nata_adapter.py'
-- class: listed: Nata raised an error in an operand next to a NULL; DuckDB did not evaluate it (dialect 9b)
-- difference: target raised an error but the oracle answered: error: execution error: a VARCHAR value does not fit INTEGER in this conversion. Use a wider type, or restrict the rows (line 1, column 122)
SET TimeZone='UTC';
CREATE TABLE t1 (id INTEGER NOT NULL, a INTEGER, b BIGINT, ti TINYINT, sm SMALLINT, c DOUBLE, r REAL, d DECIMAL(10,2), w DECIMAL(25,5), s VARCHAR, f BOOLEAN, dt DATE);
CREATE TABLE t2 (id INTEGER NOT NULL, a INTEGER, b BIGINT, ti TINYINT, sm SMALLINT, c DOUBLE, r REAL, d DECIMAL(10,2), w DECIMAL(25,5), s VARCHAR, f BOOLEAN, dt DATE);
INSERT INTO t2 VALUES (12, -1, 3, 1, 7, CAST('1e-308' AS DOUBLE), 2.0, 0.10, 12345678901234567890.12345, 'Straße', true, DATE '2021-03-14');
-- shrunk query:
SELECT (NULL // CAST('9999999999' AS INTEGER)) AS c0 FROM t2;
