SELECT s, sum(i) AS n FROM '{data}/sqlcheck/types.parquet' GROUP BY s HAVING s IS NULL OR s = 'a';
