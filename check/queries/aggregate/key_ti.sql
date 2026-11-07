SELECT ti, count(*) AS n FROM '{data}/sqlcheck/types.parquet' GROUP BY ti;
