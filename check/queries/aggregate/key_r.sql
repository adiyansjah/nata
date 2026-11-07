SELECT r, count(*) AS n FROM '{data}/sqlcheck/types.parquet' GROUP BY r;
