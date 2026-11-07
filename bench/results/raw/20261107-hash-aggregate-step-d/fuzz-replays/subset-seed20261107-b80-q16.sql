-- replay: python3 -I fuzz.py replay-subset --seed 20261107 --batch 80 --query 16 --target-cmd 'python3 -I adapters/nata_adapter.py'
-- class: listed: a select list of literals that fails (a CAST, an overflow) is an error in DuckDB over its in-memory tables even when no row is selected (planning folds it); Nata raises it only for a selected row (dialect 9b)
-- difference: oracle raised an error but the target answered: Conversion Error: Could not convert string '9999999999' to INT32 [Nata answered with 0 rows]
SET TimeZone='UTC';
CREATE TABLE t1 (id INTEGER NOT NULL, a INTEGER, b BIGINT, ti TINYINT, sm SMALLINT, c DOUBLE, r REAL, d DECIMAL(10,2), w DECIMAL(25,5), s VARCHAR, f BOOLEAN, dt DATE);
CREATE TABLE t2 (id INTEGER NOT NULL, a INTEGER, b BIGINT, ti TINYINT, sm SMALLINT, c DOUBLE, r REAL, d DECIMAL(10,2), w DECIMAL(25,5), s VARCHAR, f BOOLEAN, dt DATE);
-- shrunk query:
SELECT (- CAST('9999999999' AS INTEGER)) AS c2 FROM t2 WHERE NULL;
