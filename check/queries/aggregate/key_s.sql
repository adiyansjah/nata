SELECT s, count(*) AS n FROM '{data}/sqlcheck/types.parquet' GROUP BY s;
