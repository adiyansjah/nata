SELECT b, count(*) AS n FROM '{data}/sqlcheck/types.parquet' GROUP BY b;
