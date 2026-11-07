SELECT f, count(*) AS n FROM '{data}/sqlcheck/types.parquet' WHERE false GROUP BY f;
