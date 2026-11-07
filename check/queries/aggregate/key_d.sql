SELECT d, count(*) AS n FROM '{data}/sqlcheck/types.parquet' GROUP BY d;
