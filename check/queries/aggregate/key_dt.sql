SELECT dt, count(*) AS n FROM '{data}/sqlcheck/types.parquet' GROUP BY dt;
