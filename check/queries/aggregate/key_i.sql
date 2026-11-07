SELECT i, count(*) AS n FROM '{data}/sqlcheck/types.parquet' GROUP BY i;
