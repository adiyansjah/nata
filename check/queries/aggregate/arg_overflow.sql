-- expect-error
SELECT sum(CASE WHEN i = 2147483647 THEN i + 1 ELSE i END) AS s FROM '{data}/sqlcheck/types.parquet';
