SELECT id, i * 2 AS a, dec52 + 1 AS b, s IS NULL AS c, CASE WHEN f THEN 1 ELSE 0 END AS d, CAST(dec183 AS DOUBLE) AS e FROM '{data}/sqlcheck/dups.parquet'
