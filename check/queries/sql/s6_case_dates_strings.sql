SELECT id, CASE WHEN f THEN dt ELSE DATE '2000-01-01' END AS a, CASE WHEN f THEN s ELSE 'z' END AS b FROM '{data}/sqlcheck/types.parquet'
