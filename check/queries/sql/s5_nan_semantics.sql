SELECT id, d = CAST('NaN' AS DOUBLE) AS a, d > CAST('Infinity' AS DOUBLE) AS b, d < CAST('NaN' AS DOUBLE) AS c, d >= CAST('NaN' AS DOUBLE) AS dd FROM '{data}/sqlcheck/types.parquet'
