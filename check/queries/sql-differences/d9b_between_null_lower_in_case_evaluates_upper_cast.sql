-- expect-error-nata: a BIGINT value does not fit INTEGER
SELECT CASE WHEN i BETWEEN 2147483647 AND CAST(9223372036854775807 AS INTEGER) THEN 1 ELSE 0 END AS x FROM '{data}/sqlcheck/types.parquet' WHERE id = 6
