SELECT si, count(*) AS n FROM '{data}/sqlcheck/types.parquet' GROUP BY si;
